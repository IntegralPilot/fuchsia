// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.device/cpp/wire.h>
#include <fidl/fuchsia.driver.test/cpp/wire.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <fuchsia/hardware/block/driver/c/banjo.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/incoming/cpp/clone.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/device-watcher/cpp/device-watcher.h>
#include <lib/fdio/cpp/caller.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/namespace.h>
#include <lib/fit/function.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <lib/zx/channel.h>
#include <lib/zx/fifo.h>
#include <lib/zx/vmo.h>
#include <poll.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <threads.h>
#include <time.h>
#include <unistd.h>
#include <utime.h>
#include <zircon/errors.h>
#include <zircon/status.h>
#include <zircon/syscalls.h>

#include <algorithm>
#include <iterator>
#include <limits>
#include <memory>
#include <utility>

#include <bind/fuchsia/platform/cpp/bind.h>
#include <fbl/algorithm.h>
#include <fbl/auto_lock.h>
#include <fbl/ref_counted.h>
#include <fbl/ref_ptr.h>
#include <fbl/string.h>
#include <fbl/unique_fd.h>
#include <fbl/vector.h>
#include <zxtest/zxtest.h>

#include "lib/fidl/cpp/wire/internal/transport_channel.h"
#include "src/storage/blobfs/format.h"
#include "src/storage/fvm/format.h"
#include "src/storage/fvm/fvm_check.h"
#include "src/storage/fvm/fvm_test_instance.h"
#include "src/storage/lib/block_client/cpp/client.h"
#include "src/storage/lib/block_client/cpp/remote_block_device.h"
#include "src/storage/lib/fs_management/cpp/admin.h"
#include "src/storage/lib/fs_management/cpp/fvm.h"
#include "src/storage/lib/fs_management/cpp/mount.h"
#include "src/storage/minfs/format.h"

#define STRLEN(s) (sizeof(s) / sizeof((s)[0]))

namespace {

using VolumeManagerInfo = fuchsia_storage_block::wire::VolumeManagerInfo;

constexpr char kMountPath[] = "/test/minfs_test_mountpath";

// Returns the number of usable slices for a standard layout on a given-sized device.
size_t UsableSlicesCount(size_t disk_size, size_t slice_size) {
  return fvm::Header::FromDiskSize(fvm::kMaxUsablePartitions, disk_size, slice_size)
      .GetAllocationTableUsedEntryCount();
}

class FvmTest : public zxtest::Test {
 protected:
  fidl::ClientEnd<fuchsia_storage_block::Block> ramdisk_block_interface() const {
    return instance_->GetRamdiskPartition();
  }

  void FVMRebind() { instance_->RestartFvm(); }

  void StartFVM() { instance_->StartFvm(); }

  void CreateFVM(uint64_t block_size, uint64_t block_count, uint64_t slice_size) {
    instance_->CreateFvm(block_size, block_count, slice_size);
  }

  void CreateRamdisk(uint64_t block_size, uint64_t block_count) {
    instance_->CreateRamdisk(block_size, block_count);
  }

  VolumeManagerInfo GetFvmInfo() const { return instance_->GetFvmInfo(); }

  void FVMCheckSliceSize(size_t expected_slice_size) const;

  void FVMCheckAllocatedCount(size_t expected_allocated, size_t expected_total) const;

  zx::result<std::unique_ptr<fvm::BlockConnector>> WaitForPartition(std::string_view label) const {
    return instance_->OpenPartition(label);
  }

  zx::result<std::unique_ptr<fvm::BlockConnector>> AllocatePartition(
      fvm::AllocatePartitionRequest request) const {
    return instance_->AllocatePartition(request);
  }

  void DestroyPartition(std::string_view label) const { instance_->DestroyPartition(label); }

 private:
  std::unique_ptr<fvm::FvmInstance> instance_ = std::make_unique<fvm::FvmInstance>();
};

void FvmTest::FVMCheckSliceSize(size_t expected_slice_size) const {
  VolumeManagerInfo info = GetFvmInfo();
  ASSERT_EQ(info.slice_size, expected_slice_size, "Unexpected slice size\n");
}

void FvmTest::FVMCheckAllocatedCount(size_t expected_allocated, size_t expected_total) const {
  VolumeManagerInfo info = GetFvmInfo();
  ASSERT_EQ(info.slice_count, expected_total);
  ASSERT_EQ(info.assigned_slice_count, expected_allocated);
}

enum class ValidationResult {
  Valid,
  Corrupted,
};

void ValidateFVM(fidl::UnownedClientEnd<fuchsia_storage_block::Block> device,
                 ValidationResult expected_result = ValidationResult::Valid) {
  const fidl::WireResult result = fidl::WireCall(device)->GetInfo();
  ASSERT_OK(result.status());
  const fit::result response = result.value();
  ASSERT_TRUE(response.is_ok(), "%s", zx_status_get_string(response.error_value()));
  const fuchsia_storage_block::wire::BlockInfo& block_info = response.value()->info;
  fvm::Checker checker(device, block_info.block_size, true);
  switch (expected_result) {
    case ValidationResult::Valid:
      ASSERT_TRUE(checker.Validate());
      break;
    case ValidationResult::Corrupted:
      ASSERT_FALSE(checker.Validate());
      break;
  }
}

/////////////////////// Helper functions, definitions

constexpr uuid::Uuid kTestUniqueGuid1 = {0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07,
                                         0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f};
constexpr uuid::Uuid kTestUniqueGuid2 = {0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17,
                                         0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f};

// Intentionally avoid aligning these GUIDs with
// the actual system GUIDs; otherwise, limited versions
// of Fuchsia may attempt to actually mount these
// partitions automatically.

constexpr std::string_view kTestPartDataName = "data";
constexpr uuid::Uuid kTestPartDataGuid = {
    0xAA, 0xFF, 0xBB, 0x00, 0x33, 0x44, 0x88, 0x99, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17,
};

constexpr std::string_view kTestPartBlobName = "blob";
constexpr uuid::Uuid kTestPartBlobGuid = {
    0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0xAA, 0xFF, 0xBB, 0x00, 0x33, 0x44, 0x88, 0x99,
};

constexpr std::string_view kTestPartSystemName = "system";
constexpr uuid::Uuid kTestPartSystemGuid = {
    0xEE, 0xFF, 0xBB, 0x00, 0x33, 0x44, 0x88, 0x99, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17,
};

class VmoBuf;

class VmoClient : public fbl::RefCounted<VmoClient> {
 public:
  explicit VmoClient(fidl::ClientEnd<fuchsia_storage_block::Block> device);
  ~VmoClient() = default;

  void CheckWrite(VmoBuf& vbuf, size_t buf_off, size_t dev_off, size_t len);
  void CheckRead(VmoBuf& vbuf, size_t buf_off, size_t dev_off, size_t len);
  void Transaction(BlockFifoRequest* requests, size_t count) {
    ASSERT_OK(client_->Transaction(requests, count));
  }
  zx::result<storage::OwnedVmoid> RegisterVmo(const zx::vmo& vmo) {
    return client_->RegisterVmo(vmo);
  }

  fidl::UnownedClientEnd<fuchsia_storage_block::Block> device() const { return device_.borrow(); }

  static groupid_t group() { return 0; }

 private:
  fidl::ClientEnd<fuchsia_storage_block::Block> device_;
  uint32_t block_size_;
  std::unique_ptr<block_client::Client> client_;
};

class VmoBuf {
 public:
  VmoBuf(fbl::RefPtr<VmoClient> client, size_t size) : client_(std::move(client)) {
    buf_ = std::make_unique<uint8_t[]>(size);

    ASSERT_OK(zx::vmo::create(size, 0, &vmo_));
    zx::result vmoid = client_->RegisterVmo(vmo_);
    ASSERT_OK(vmoid);
    vmoid_ = std::move(vmoid.value());
  }

  ~VmoBuf() {
    if (vmo_.is_valid()) {
      BlockFifoRequest request = {
          .command = {.opcode = BLOCK_OPCODE_CLOSE_VMO, .flags = 0},
          .group = client_->group(),
          .vmoid = vmoid_.TakeId(),
      };
      client_->Transaction(&request, 1);
    }
  }

 private:
  friend VmoClient;

  fbl::RefPtr<VmoClient> client_;
  zx::vmo vmo_;
  std::unique_ptr<uint8_t[]> buf_;
  storage::OwnedVmoid vmoid_;
};

VmoClient::VmoClient(fidl::ClientEnd<fuchsia_storage_block::Block> device)
    : device_(std::move(device)) {
  {
    const fidl::WireResult result = fidl::WireCall(device_.borrow())->GetInfo();
    ASSERT_OK(result.status());
    const fit::result response = result.value();
    ASSERT_TRUE(response.is_ok(), "%s", zx_status_get_string(response.error_value()));
    block_size_ = response.value()->info.block_size;
  }

  auto [session, server] = fidl::Endpoints<fuchsia_storage_block::Session>::Create();

  const fidl::Status result = fidl::WireCall(device_.borrow())->OpenSession(std::move(server));
  ASSERT_OK(result.status());

  const fidl::WireResult fifo_result = fidl::WireCall(session)->GetFifo();
  ASSERT_OK(fifo_result.status());
  const fit::result fifo_response = fifo_result.value();
  ASSERT_TRUE(fifo_response.is_ok(), "%s", zx_status_get_string(fifo_response.error_value()));

  client_ = std::make_unique<block_client::Client>(std::move(session),
                                                   std::move(fifo_response.value()->fifo));
}

void VmoClient::CheckWrite(VmoBuf& vbuf, size_t buf_off, size_t dev_off, size_t len) {
  // Write to the client-side buffer
  for (size_t i = 0; i < len; i++)
    vbuf.buf_[i + buf_off] = static_cast<uint8_t>(rand());

  // Write to the registered VMO
  ASSERT_OK(vbuf.vmo_.write(&vbuf.buf_[buf_off], buf_off, len));
  ASSERT_EQ(len % block_size_, 0);
  ASSERT_EQ(buf_off % block_size_, 0);
  ASSERT_EQ(dev_off % block_size_, 0);

  // Write to the block device
  BlockFifoRequest request = {
      .command = {.opcode = BLOCK_OPCODE_WRITE, .flags = 0},
      .group = group(),
      .vmoid = vbuf.vmoid_.get(),
      .length = static_cast<uint32_t>(len / block_size_),
      .vmo_offset = buf_off / block_size_,
      .dev_offset = dev_off / block_size_,
  };
  Transaction(&request, 1);
}

void VmoClient::CheckRead(VmoBuf& vbuf, size_t buf_off, size_t dev_off, size_t len) {
  // Create a comparison buffer
  fbl::AllocChecker ac;
  std::unique_ptr<uint8_t[]> out(new (&ac) uint8_t[len]);
  ASSERT_TRUE(ac.check());
  memset(out.get(), 0, len);

  ASSERT_EQ(len % block_size_, 0);
  ASSERT_EQ(buf_off % block_size_, 0);
  ASSERT_EQ(dev_off % block_size_, 0);

  // Read from the block device
  BlockFifoRequest request = {
      .command = {.opcode = BLOCK_OPCODE_READ, .flags = 0},
      .group = group(),
      .vmoid = vbuf.vmoid_.get(),
      .length = static_cast<uint32_t>(len / block_size_),
      .vmo_offset = buf_off / block_size_,
      .dev_offset = dev_off / block_size_,
  };
  Transaction(&request, 1);

  // Read from the registered VMO
  ASSERT_OK(vbuf.vmo_.read(out.get(), buf_off, len));

  ASSERT_EQ(memcmp(&vbuf.buf_[buf_off], out.get(), len), 0);
}

void CheckWrite(fidl::UnownedClientEnd<fuchsia_storage_block::Block> device, size_t off, size_t len,
                uint8_t* buf) {
  for (size_t i = 0; i < len; i++) {
    buf[i] = static_cast<uint8_t>(rand());
  }
  ASSERT_OK(block_client::SingleWriteBytes(device, buf, len, off));
}

void CheckRead(fidl::UnownedClientEnd<fuchsia_storage_block::Block> device, size_t off, size_t len,
               const uint8_t* in) {
  fbl::AllocChecker ac;
  std::unique_ptr<uint8_t[]> out(new (&ac) uint8_t[len]);
  ASSERT_TRUE(ac.check());
  memset(out.get(), 0, len);
  ASSERT_OK(block_client::SingleReadBytes(device, out.get(), len, off));
  ASSERT_EQ(memcmp(in, out.get(), len), 0);
}

void CheckWriteReadBlock(fidl::UnownedClientEnd<fuchsia_storage_block::Block> device, size_t block,
                         size_t count) {
  const fidl::WireResult result = fidl::WireCall(device)->GetInfo();
  ASSERT_OK(result.status());
  const fit::result response = result.value();
  ASSERT_TRUE(response.is_ok(), "%s", zx_status_get_string(response.error_value()));
  const fuchsia_storage_block::wire::BlockInfo& block_info = response.value()->info;
  size_t len = block_info.block_size * count;
  size_t off = block_info.block_size * block;
  std::unique_ptr<uint8_t[]> in(new uint8_t[len]);
  ASSERT_NO_FATAL_FAILURE(CheckWrite(device, off, len, in.get()));
  ASSERT_NO_FATAL_FAILURE(CheckRead(device, off, len, in.get()));
}

void CheckWriteReadBytesFifo(fidl::UnownedClientEnd<fuchsia_storage_block::Block> device,
                             size_t off, size_t len) {
  std::unique_ptr<uint8_t[]> write_buf(new uint8_t[len]);
  memset(write_buf.get(), 0xa3, len);

  ASSERT_OK(block_client::SingleWriteBytes(device, write_buf.get(), len, off));
  std::unique_ptr<uint8_t[]> read_buf(new uint8_t[len]);
  memset(read_buf.get(), 0, len);
  ASSERT_OK(block_client::SingleReadBytes(device, read_buf.get(), len, off));
  EXPECT_EQ(memcmp(write_buf.get(), read_buf.get(), len), 0);
}

void CheckNoAccessBlock(fidl::UnownedClientEnd<fuchsia_storage_block::Block> device, size_t block,
                        size_t count) {
  const fidl::WireResult result = fidl::WireCall(device)->GetInfo();
  ASSERT_OK(result.status());
  const fit::result response = result.value();
  ASSERT_TRUE(response.is_ok(), "%s", zx_status_get_string(response.error_value()));
  const fuchsia_storage_block::wire::BlockInfo& block_info = response.value()->info;
  size_t len = block_info.block_size * count;
  size_t off = block_info.block_size * block;
  std::unique_ptr<uint8_t[]> buf(new uint8_t[len]);
  for (size_t i = 0; i < len; i++) {
    buf[i] = static_cast<uint8_t>(rand());
  }
  ASSERT_STATUS(block_client::SingleWriteBytes(device, buf.get(), len, off), ZX_ERR_OUT_OF_RANGE);
  ASSERT_STATUS(block_client::SingleReadBytes(device, buf.get(), len, off), ZX_ERR_OUT_OF_RANGE);
}

void CheckDeadConnection(const zx::unowned_channel& chan) {
  ASSERT_OK(chan->wait_one(ZX_CHANNEL_PEER_CLOSED, zx::time::infinite(), nullptr));
}

/////////////////////// Actual tests:

// Test initializing the FVM on a partition that is smaller than a slice
TEST_F(FvmTest, TestTooSmall) {
  uint64_t block_size = 512;
  uint64_t block_count = (1 << 15);

  CreateRamdisk(block_size, block_count);
  size_t slice_size = block_size * block_count;
  auto ramdisk = ramdisk_block_interface();
  ASSERT_EQ(fs_management::FvmInit(ramdisk.borrow(), slice_size), ZX_ERR_NO_SPACE);
  ValidateFVM(ramdisk.borrow(), ValidationResult::Corrupted);
}

// Test initializing the FVM on a large partition, with metadata size > the max transfer size
TEST_F(FvmTest, TestLarge) {
  constexpr uint64_t kBlockSize = 512;
  constexpr uint64_t kBlockCount{UINT64_C(8) * (1 << 20)};
  CreateRamdisk(kBlockSize, kBlockCount);

  constexpr size_t kSliceSize{static_cast<size_t>(16) * (1 << 10)};
  fvm::Header fvm_header =
      fvm::Header::FromDiskSize(fvm::kMaxUsablePartitions, kBlockSize * kBlockCount, kSliceSize);

  auto ramdisk = ramdisk_block_interface();
  const fidl::WireResult result = fidl::WireCall(ramdisk.borrow())->GetInfo();
  ASSERT_OK(result.status());
  const fit::result response = result.value();
  ASSERT_TRUE(response.is_ok(), "%s", zx_status_get_string(response.error_value()));
  const fuchsia_storage_block::wire::BlockInfo& block_info = response.value()->info;
  ASSERT_LT(block_info.max_transfer_size, fvm_header.GetMetadataAllocatedBytes());

  ASSERT_OK(fs_management::FvmInit(ramdisk.borrow(), kSliceSize));

  StartFVM();
  ValidateFVM(ramdisk.borrow());
}

// Load and unload an empty FVM
TEST_F(FvmTest, TestEmpty) {
  constexpr uint64_t kBlockSize = 512;
  constexpr uint64_t kBlockCount = 1 << 16;
  constexpr uint64_t kSliceSize = 64 * kBlockSize;
  CreateFVM(kBlockSize, kBlockCount, kSliceSize);
  FVMCheckSliceSize(kSliceSize);
  ValidateFVM(ramdisk_block_interface().borrow());
}

// Test allocating a single partition
TEST_F(FvmTest, TestAllocateOne) {
  constexpr uint64_t kBlockSize = 512;
  constexpr uint64_t kBlockCount = 1 << 16;
  constexpr uint64_t kSliceSize = 64 * kBlockSize;
  CreateFVM(kBlockSize, kBlockCount, kSliceSize);

  // Allocate one VPart
  zx::result vp_or = AllocatePartition({
      .type = kTestPartDataGuid,
      .guid = kTestUniqueGuid1,
      .name = kTestPartDataName,
  });
  ASSERT_OK(vp_or);
  std::unique_ptr<fvm::BlockConnector> vp = std::move(vp_or.value());

  // Check that the name matches what we provided
  const fidl::WireResult result = fidl::WireCall(vp->as_block())->GetName();
  ASSERT_OK(result.status());
  const fidl::WireResponse response = result.value();
  ASSERT_OK(response.status);
  ASSERT_STREQ(response.name.get(), kTestPartDataName.data());

  // Check that we can read from / write to it.
  CheckWriteReadBlock(vp->as_block(), 0, 1);

  // Try accessing the block again after closing / re-opening it.
  vp = {};
  vp_or = WaitForPartition(kTestPartDataName);
  ASSERT_OK(vp_or, "Couldn't re-open Data VPart");
  vp = std::move(vp_or.value());
  CheckWriteReadBlock(vp->as_block(), 0, 1);
  vp = {};

  FVMCheckSliceSize(kSliceSize);
  ValidateFVM(ramdisk_block_interface().borrow());
}

// Test Reading and writing with RemoteBlockDevice helpers
TEST_F(FvmTest, TestReadWriteSingle) {
  constexpr uint64_t kBlockSize = 512;
  constexpr uint64_t kBlockCount = 1 << 16;
  constexpr uint64_t kSliceSize = 64 * kBlockSize;
  CreateFVM(kBlockSize, kBlockCount, kSliceSize);

  // Allocate one VPart
  zx::result vp = AllocatePartition({
      .type = kTestPartDataGuid,
      .guid = kTestUniqueGuid1,
      .name = kTestPartDataName,
  });
  ASSERT_OK(vp);

  // Check that we can read from / write to it.
  CheckWriteReadBytesFifo(vp->as_block(), 0, kBlockSize);
  // Check with an offset
  CheckWriteReadBytesFifo(vp->as_block(), kBlockSize * 7, kBlockSize * 4);
  vp = {};

  FVMCheckSliceSize(kSliceSize);
  ValidateFVM(ramdisk_block_interface().borrow());
}

// Test allocating a collection of partitions
TEST_F(FvmTest, TestAllocateMany) {
  constexpr uint64_t kBlockSize = 512;
  constexpr uint64_t kBlockCount = 1 << 16;
  constexpr uint64_t kSliceSize = 64 * kBlockSize;
  CreateFVM(kBlockSize, kBlockCount, kSliceSize);

  // Test allocation of multiple VPartitions
  zx::result data_or = AllocatePartition({
      .type = kTestPartDataGuid,
      .guid = kTestUniqueGuid1,
      .name = kTestPartDataName,
  });
  ASSERT_OK(data_or);
  std::unique_ptr<fvm::BlockConnector> data = *std::move(data_or);

  zx::result blob_or = AllocatePartition({
      .type = kTestPartBlobGuid,
      .guid = kTestUniqueGuid1,
      .name = kTestPartBlobName,
  });
  ASSERT_OK(blob_or);
  std::unique_ptr<fvm::BlockConnector> blob = *std::move(blob_or);

  zx::result sys_or = AllocatePartition({
      .type = kTestPartSystemGuid,
      .guid = kTestUniqueGuid1,
      .name = kTestPartSystemName,
  });
  ASSERT_OK(sys_or);
  std::unique_ptr<fvm::BlockConnector> sys = *std::move(sys_or);

  CheckWriteReadBlock(data->as_block(), 0, 1);
  CheckWriteReadBlock(blob->as_block(), 0, 1);
  CheckWriteReadBlock(sys->as_block(), 0, 1);

  data = {};
  blob = {};
  sys = {};

  FVMCheckSliceSize(kSliceSize);
  ValidateFVM(ramdisk_block_interface().borrow());
}

// Test allocating additional slices to a vpartition.
TEST_F(FvmTest, TestVPartitionExtend) {
  constexpr uint64_t kBlockSize = 512;
  constexpr uint64_t kBlockCount = 1 << 16;
  constexpr uint64_t kSliceSize = 64 * kBlockSize;
  CreateFVM(kBlockSize, kBlockCount, kSliceSize);

  VolumeManagerInfo info = GetFvmInfo();
  size_t slice_size = info.slice_size;
  constexpr uint64_t kDiskSize = kBlockSize * kBlockCount;
  size_t slices_total = UsableSlicesCount(kDiskSize, slice_size);
  size_t slices_left = slices_total;

  FVMCheckAllocatedCount(slices_total - slices_left, slices_total);

  // Allocate one VPart
  size_t slice_count = 1;
  auto vp_or = AllocatePartition({
      .slice_count = slice_count,
      .type = kTestPartDataGuid,
      .guid = kTestUniqueGuid1,
      .name = kTestPartDataName,
  });
  ASSERT_OK(vp_or, "Couldn't open Volume");
  std::unique_ptr<fvm::BlockConnector> vp = *std::move(vp_or);
  slices_left--;
  FVMCheckAllocatedCount(slices_total - slices_left, slices_total);

  // Confirm that the disk reports the correct number of slices
  {
    const fidl::WireResult result = fidl::WireCall(vp->as_block())->GetInfo();
    ASSERT_OK(result.status());
    const fit::result response = result.value();
    ASSERT_TRUE(response.is_ok(), "%s", zx_status_get_string(response.error_value()));
    const fuchsia_storage_block::wire::BlockInfo& block_info = response.value()->info;
    ASSERT_EQ(block_info.block_count * block_info.block_size, slice_size * slice_count);
  }

  // Try re-allocating an already allocated vslice
  {
    const fidl::WireResult result = fidl::WireCall(vp->as_block())->Extend(0, 1);
    ASSERT_OK(result.status());
    const fidl::WireResponse response = result.value();
    ASSERT_STATUS(response.status, ZX_ERR_OUT_OF_RANGE);
  }

  {
    const fidl::WireResult result = fidl::WireCall(vp->as_block())->GetInfo();
    ASSERT_OK(result.status());
    const fit::result response = result.value();
    ASSERT_TRUE(response.is_ok(), "%s", zx_status_get_string(response.error_value()));
    const fuchsia_storage_block::wire::BlockInfo& block_info = response.value()->info;
    ASSERT_EQ(block_info.block_count * block_info.block_size, slice_size * slice_count);
  }

  // Try again with a portion of the request which is unallocated
  {
    const fidl::WireResult result = fidl::WireCall(vp->as_block())->Extend(0, 2);
    ASSERT_OK(result.status());
    const fidl::WireResponse response = result.value();
    ASSERT_STATUS(response.status, ZX_ERR_OUT_OF_RANGE);
  }

  // Allocate OBSCENELY too many slices
  {
    const fidl::WireResult result =
        fidl::WireCall(vp->as_block())->Extend(slice_count, std::numeric_limits<uint64_t>::max());
    ASSERT_OK(result.status());
    const fidl::WireResponse response = result.value();
    ASSERT_STATUS(response.status, ZX_ERR_OUT_OF_RANGE);
  }

  // Allocate slices at a too-large offset
  {
    const fidl::WireResult result =
        fidl::WireCall(vp->as_block())->Extend(std::numeric_limits<uint64_t>::max(), 1);
    ASSERT_OK(result.status());
    const fidl::WireResponse response = result.value();
    ASSERT_STATUS(response.status, ZX_ERR_OUT_OF_RANGE);
  }

  // Attempt to allocate slightly too many slices
  {
    const fidl::WireResult result =
        fidl::WireCall(vp->as_block())->Extend(slice_count, slices_left + 1);
    ASSERT_OK(result.status());
    const fidl::WireResponse response = result.value();
    ASSERT_STATUS(response.status, ZX_ERR_NO_SPACE);
  }

  // The number of free slices should be unchanged.
  FVMCheckAllocatedCount(slices_total - slices_left, slices_total);

  // Allocate exactly the remaining number of slices
  {
    const fidl::WireResult result =
        fidl::WireCall(vp->as_block())->Extend(slice_count, slices_left);
    ASSERT_OK(result.status());
    const fidl::WireResponse response = result.value();
    ASSERT_OK(response.status);
  }

  slice_count += slices_left;
  slices_left = 0;
  FVMCheckAllocatedCount(slices_total - slices_left, slices_total);

  {
    const fidl::WireResult result = fidl::WireCall(vp->as_block())->GetInfo();
    ASSERT_OK(result.status());
    const fit::result response = result.value();
    ASSERT_TRUE(response.is_ok(), "%s", zx_status_get_string(response.error_value()));
    const fuchsia_storage_block::wire::BlockInfo& block_info = response.value()->info;
    ASSERT_EQ(block_info.block_count * block_info.block_size, slice_size * slice_count);
  }

  // We can't allocate any more to this VPartition
  {
    const fidl::WireResult result = fidl::WireCall(vp->as_block())->Extend(slice_count, 1);
    ASSERT_OK(result.status());
    const fidl::WireResponse response = result.value();
    ASSERT_STATUS(response.status, ZX_ERR_NO_SPACE);
  }

  // We can't allocate a new VPartition
  zx::result vp2_or = AllocatePartition({
      .type = kTestPartBlobGuid,
      .guid = kTestUniqueGuid2,
      .name = kTestPartBlobName,
  });
  ASSERT_NOT_OK(vp2_or, "Expected VPart allocation failure");

  FVMCheckSliceSize(kSliceSize);
  ValidateFVM(ramdisk_block_interface().borrow());
}

// Test allocating very sparse VPartition
TEST_F(FvmTest, TestVPartitionExtendSparse) {
  constexpr uint64_t kBlockSize = 512;
  constexpr uint64_t kBlockCount = 1 << 16;
  constexpr uint64_t kSliceSize = 64 * kBlockSize;
  CreateFVM(kBlockSize, kBlockCount, kSliceSize);

  zx::result vp_or = AllocatePartition({
      .type = kTestPartDataGuid,
      .guid = kTestUniqueGuid1,
      .name = kTestPartDataName,
  });
  ASSERT_OK(vp_or);
  std::unique_ptr<fvm::BlockConnector> vp = *std::move(vp_or);
  CheckWriteReadBlock(vp->as_block(), 0, 1);

  // Double check that we can access a block at this vslice address
  // (this isn't always possible; for certain slice sizes, blocks may be
  // allocatable / freeable, but not addressable).
  size_t bno = (fvm::kMaxVSlices - 1) * (kSliceSize / kBlockSize);
  ASSERT_EQ(bno / (kSliceSize / kBlockSize), (fvm::kMaxVSlices - 1), "bno overflowed");
  ASSERT_EQ((bno * kBlockSize) / kBlockSize, bno, "block access will overflow");

  // Try allocating at a location that's slightly too large
  {
    const fidl::WireResult result = fidl::WireCall(vp->as_block())->Extend(fvm::kMaxVSlices, 1);
    ASSERT_OK(result.status());
    const fidl::WireResponse response = result.value();
    ASSERT_STATUS(response.status, ZX_ERR_OUT_OF_RANGE);
  }

  // Try allocating at the largest offset
  {
    const fidl::WireResult result = fidl::WireCall(vp->as_block())->Extend(fvm::kMaxVSlices - 1, 1);
    ASSERT_OK(result.status());
    const fidl::WireResponse response = result.value();
    ASSERT_OK(response.status);
  }

  CheckWriteReadBlock(vp->as_block(), bno, 1);

  // Try freeing beyond largest offset
  {
    const fidl::WireResult result = fidl::WireCall(vp->as_block())->Shrink(fvm::kMaxVSlices, 1);
    ASSERT_OK(result.status());
    const fidl::WireResponse response = result.value();
    ASSERT_STATUS(response.status, ZX_ERR_OUT_OF_RANGE);
  }

  CheckWriteReadBlock(vp->as_block(), bno, 1);

  // Try freeing at the largest offset
  {
    const fidl::WireResult result = fidl::WireCall(vp->as_block())->Shrink(fvm::kMaxVSlices - 1, 1);
    ASSERT_OK(result.status());
    const fidl::WireResponse response = result.value();
    ASSERT_OK(response.status);
  }

  CheckNoAccessBlock(vp->as_block(), bno, 1);

  vp = {};
  FVMCheckSliceSize(kSliceSize);
  ValidateFVM(ramdisk_block_interface().borrow());
}

// Test removing slices from a VPartition.
TEST_F(FvmTest, TestVPartitionShrink) {
  constexpr uint64_t kBlockSize = 512;
  constexpr uint64_t kBlockCount = 1 << 16;
  constexpr uint64_t kSliceSize = 64 * kBlockSize;
  CreateFVM(kBlockSize, kBlockCount, kSliceSize);

  VolumeManagerInfo info = GetFvmInfo();
  size_t slice_size = info.slice_size;
  const size_t kDiskSize = kBlockSize * kBlockCount;
  size_t slices_total = UsableSlicesCount(kDiskSize, slice_size);
  size_t slices_left = slices_total;

  FVMCheckAllocatedCount(slices_total - slices_left, slices_total);

  // Allocate one VPart
  size_t slice_count = 1;
  zx::result vp_or = AllocatePartition({
      .slice_count = slice_count,
      .type = kTestPartDataGuid,
      .guid = kTestUniqueGuid1,
      .name = kTestPartDataName,
  });
  ASSERT_OK(vp_or, "Couldn't open Volume");
  std::unique_ptr<fvm::BlockConnector> vp = *std::move(vp_or);
  slices_left--;

  // Confirm that the disk reports the correct number of slices
  {
    const fidl::WireResult result = fidl::WireCall(vp->as_block())->GetInfo();
    ASSERT_OK(result.status());
    const fit::result response = result.value();
    ASSERT_TRUE(response.is_ok(), "%s", zx_status_get_string(response.error_value()));
    const fuchsia_storage_block::wire::BlockInfo& block_info = response.value()->info;
    ASSERT_EQ(block_info.block_count * block_info.block_size, slice_size * slice_count);
    CheckWriteReadBlock(vp->as_block(), (slice_size / block_info.block_size) - 1, 1);
    CheckNoAccessBlock(vp->as_block(), (slice_size / block_info.block_size) - 1, 2);
    FVMCheckAllocatedCount(slices_total - slices_left, slices_total);
  }

  // Try shrinking the 0th vslice
  {
    const fidl::WireResult result = fidl::WireCall(vp->as_block())->Shrink(0, 1);
    ASSERT_OK(result.status());
    const fidl::WireResponse response = result.value();
    ASSERT_STATUS(response.status, ZX_ERR_OUT_OF_RANGE);
  }

  // Try no-op requests (length = 0).
  {
    const fidl::WireResult result = fidl::WireCall(vp->as_block())->Extend(1, 0);
    ASSERT_OK(result.status());
    const fidl::WireResponse response = result.value();
    ASSERT_OK(response.status);
  }
  {
    const fidl::WireResult result = fidl::WireCall(vp->as_block())->Shrink(1, 0);
    ASSERT_OK(result.status());
    const fidl::WireResponse response = result.value();
    ASSERT_OK(response.status);
  }

  {
    const fidl::WireResult result = fidl::WireCall(vp->as_block())->GetInfo();
    ASSERT_OK(result.status());
    const fit::result response = result.value();
    ASSERT_TRUE(response.is_ok(), "%s", zx_status_get_string(response.error_value()));
    const fuchsia_storage_block::wire::BlockInfo& block_info = response.value()->info;
    ASSERT_EQ(block_info.block_count * block_info.block_size, slice_size * slice_count);
  }

  // Try again with a portion of the request which is unallocated
  {
    const fidl::WireResult result = fidl::WireCall(vp->as_block())->Shrink(1, 2);
    ASSERT_OK(result.status());
    const fidl::WireResponse response = result.value();
    ASSERT_STATUS(response.status, ZX_ERR_INVALID_ARGS);
  }
  {
    const fidl::WireResult result = fidl::WireCall(vp->as_block())->GetInfo();
    ASSERT_OK(result.status());
    const fit::result response = result.value();
    ASSERT_TRUE(response.is_ok(), "%s", zx_status_get_string(response.error_value()));
    const fuchsia_storage_block::wire::BlockInfo& block_info = response.value()->info;
    ASSERT_EQ(block_info.block_count * block_info.block_size, slice_size * slice_count);
    FVMCheckAllocatedCount(slices_total - slices_left, slices_total);
  }

  // Allocate exactly the remaining number of slices
  {
    const fidl::WireResult result =
        fidl::WireCall(vp->as_block())->Extend(slice_count, slices_left);
    ASSERT_OK(result.status());
    const fidl::WireResponse response = result.value();
    ASSERT_OK(response.status);
  }
  slice_count += slices_left;
  slices_left = 0;

  {
    const fidl::WireResult result = fidl::WireCall(vp->as_block())->GetInfo();
    ASSERT_OK(result.status());
    const fit::result response = result.value();
    ASSERT_TRUE(response.is_ok(), "%s", zx_status_get_string(response.error_value()));
    const fuchsia_storage_block::wire::BlockInfo& block_info = response.value()->info;
    ASSERT_EQ(block_info.block_count * block_info.block_size, slice_size * slice_count);
    CheckWriteReadBlock(vp->as_block(), (slice_size / block_info.block_size) - 1, 1);
    CheckWriteReadBlock(vp->as_block(), (slice_size / block_info.block_size) - 1, 2);
  }
  FVMCheckAllocatedCount(slices_total - slices_left, slices_total);

  // We can't allocate any more to this VPartition
  {
    const fidl::WireResult result = fidl::WireCall(vp->as_block())->Extend(slice_count, 1);
    ASSERT_OK(result.status());
    const fidl::WireResponse response = result.value();
    ASSERT_STATUS(response.status, ZX_ERR_NO_SPACE);
  }

  // Try to shrink off the end (okay, since SOME of the slices are allocated)
  {
    const fidl::WireResult result = fidl::WireCall(vp->as_block())->Shrink(1, slice_count + 3);
    ASSERT_OK(result.status());
    const fidl::WireResponse response = result.value();
    ASSERT_OK(response.status);
  }
  FVMCheckAllocatedCount(1, slices_total);

  // The same request to shrink should now fail (NONE of the slices are
  // allocated)
  {
    const fidl::WireResult result = fidl::WireCall(vp->as_block())->Shrink(1, slice_count - 1);
    ASSERT_OK(result.status());
    const fidl::WireResponse response = result.value();
    ASSERT_STATUS(response.status, ZX_ERR_INVALID_ARGS);
  }
  FVMCheckAllocatedCount(1, slices_total);

  // ... unless we re-allocate and try again.
  {
    const fidl::WireResult result = fidl::WireCall(vp->as_block())->Extend(1, slice_count - 1);
    ASSERT_OK(result.status());
    const fidl::WireResponse response = result.value();
    ASSERT_OK(response.status);
  }
  {
    const fidl::WireResult result = fidl::WireCall(vp->as_block())->Shrink(1, slice_count - 1);
    ASSERT_OK(result.status());
    const fidl::WireResponse response = result.value();
    ASSERT_OK(response.status);
  }

  FVMCheckSliceSize(kSliceSize);
  ValidateFVM(ramdisk_block_interface().borrow());
}

// Test splitting a contiguous slice extent into multiple parts
TEST_F(FvmTest, TestVPartitionSplit) {
  constexpr uint64_t kBlockSize = 512;
  constexpr uint64_t kBlockCount = 1 << 16;
  constexpr uint64_t kSliceSize = 64 * kBlockSize;
  CreateFVM(kBlockSize, kBlockCount, kSliceSize);

  VolumeManagerInfo info = GetFvmInfo();
  size_t slice_size = info.slice_size;

  // Allocate one VPart
  size_t slice_count = 5;
  zx::result vp_or = AllocatePartition({
      .slice_count = slice_count,
      .type = kTestPartDataGuid,
      .guid = kTestUniqueGuid1,
      .name = kTestPartDataName,
  });
  ASSERT_OK(vp_or);
  std::unique_ptr<fvm::BlockConnector> vp = *std::move(vp_or);

  // Confirm that the disk reports the correct number of slices
  const fidl::WireResult result = fidl::WireCall(vp->as_block())->GetInfo();
  ASSERT_OK(result.status());
  const fit::result response = result.value();
  ASSERT_TRUE(response.is_ok(), "%s", zx_status_get_string(response.error_value()));
  const fuchsia_storage_block::wire::BlockInfo& block_info = response.value()->info;
  ASSERT_EQ(block_info.block_count * block_info.block_size, slice_size * slice_count);

  size_t reset_offset = 1;
  size_t reset_length = slice_count - 1;
  size_t mid_offset = 2;
  size_t mid_length = 1;
  size_t start_offset = 1;
  size_t start_length = 1;
  size_t end_offset = 3;
  size_t end_length = slice_count - 3;

  auto verifyExtents = [&](bool start, bool mid, bool end) {
    size_t start_block = start_offset * (slice_size / block_info.block_size);
    size_t mid_block = mid_offset * (slice_size / block_info.block_size);
    size_t end_block = end_offset * (slice_size / block_info.block_size);

    if (start) {
      CheckWriteReadBlock(vp->as_block(), start_block, 1);
    } else {
      CheckNoAccessBlock(vp->as_block(), start_block, 1);
    }
    if (mid) {
      CheckWriteReadBlock(vp->as_block(), mid_block, 1);
    } else {
      CheckNoAccessBlock(vp->as_block(), mid_block, 1);
    }
    if (end) {
      CheckWriteReadBlock(vp->as_block(), end_block, 1);
    } else {
      CheckNoAccessBlock(vp->as_block(), end_block, 1);
    }
    return true;
  };

  auto doExtend = [&vp](size_t offset, size_t length) {
    const fidl::WireResult result = fidl::WireCall(vp->as_block())->Extend(offset, length);
    ASSERT_OK(result.status());
    const fidl::WireResponse response = result.value();
    ASSERT_OK(response.status);
  };

  auto doShrink = [&vp](size_t offset, size_t length) {
    const fidl::WireResult result = fidl::WireCall(vp->as_block())->Shrink(offset, length);
    ASSERT_OK(result.status());
    const fidl::WireResponse response = result.value();
    ASSERT_OK(response.status);
  };

  // We should be able to split the extent.
  verifyExtents(true, true, true);
  doShrink(mid_offset, mid_length);
  verifyExtents(true, false, true);
  doShrink(start_offset, start_length);
  verifyExtents(false, false, true);
  doShrink(end_offset, end_length);
  verifyExtents(false, false, false);

  doExtend(reset_offset, reset_length);

  doShrink(start_offset, start_length);
  verifyExtents(false, true, true);
  doShrink(mid_offset, mid_length);
  verifyExtents(false, false, true);
  doShrink(end_offset, end_length);
  verifyExtents(false, false, false);

  doExtend(reset_offset, reset_length);

  doShrink(end_offset, end_length);
  verifyExtents(true, true, false);
  doShrink(mid_offset, mid_length);
  verifyExtents(true, false, false);
  doShrink(start_offset, start_length);
  verifyExtents(false, false, false);

  doExtend(reset_offset, reset_length);

  doShrink(end_offset, end_length);
  verifyExtents(true, true, false);
  doShrink(start_offset, start_length);
  verifyExtents(false, true, false);
  doShrink(mid_offset, mid_length);
  verifyExtents(false, false, false);

  // We should also be able to combine extents
  doExtend(mid_offset, mid_length);
  verifyExtents(false, true, false);
  doExtend(start_offset, start_length);
  verifyExtents(true, true, false);
  doExtend(end_offset, end_length);
  verifyExtents(true, true, true);

  doShrink(reset_offset, reset_length);

  doExtend(end_offset, end_length);
  verifyExtents(false, false, true);
  doExtend(mid_offset, mid_length);
  verifyExtents(false, true, true);
  doExtend(start_offset, start_length);
  verifyExtents(true, true, true);

  doShrink(reset_offset, reset_length);

  doExtend(end_offset, end_length);
  verifyExtents(false, false, true);
  doExtend(start_offset, start_length);
  verifyExtents(true, false, true);
  doExtend(mid_offset, mid_length);
  verifyExtents(true, true, true);

  vp = {};
  FVMCheckSliceSize(kSliceSize);
  ValidateFVM(ramdisk_block_interface());
}

// Test removing VPartitions within an FVM
TEST_F(FvmTest, TestVPartitionDestroy) {
  constexpr uint64_t kBlockSize = 512;
  constexpr uint64_t kBlockCount = 1 << 16;
  constexpr uint64_t kSliceSize = 64 * kBlockSize;
  CreateFVM(kBlockSize, kBlockCount, kSliceSize);

  // Test allocation of multiple VPartitions
  zx::result data_or = AllocatePartition({
      .type = kTestPartDataGuid,
      .guid = kTestUniqueGuid1,
      .name = kTestPartDataName,
  });
  ASSERT_OK(data_or);
  std::unique_ptr<fvm::BlockConnector> data = *std::move(data_or);

  zx::result blob_or = AllocatePartition({
      .type = kTestPartBlobGuid,
      .guid = kTestUniqueGuid1,
      .name = kTestPartBlobName,
  });
  ASSERT_OK(blob_or);
  std::unique_ptr<fvm::BlockConnector> blob = *std::move(blob_or);

  zx::result sys_or = AllocatePartition({
      .type = kTestPartSystemGuid,
      .guid = kTestUniqueGuid1,
      .name = kTestPartSystemName,
  });
  ASSERT_OK(sys_or);
  std::unique_ptr<fvm::BlockConnector> sys = *std::move(sys_or);

  // We can access all three...
  CheckWriteReadBlock(data->as_block(), 0, 1);
  CheckWriteReadBlock(blob->as_block(), 0, 1);
  CheckWriteReadBlock(sys->as_block(), 0, 1);

  // But not after we destroy the blob partition.
  DestroyPartition(kTestPartBlobName);
  CheckWriteReadBlock(data->as_block(), 0, 1);
  CheckWriteReadBlock(sys->as_block(), 0, 1);
  CheckDeadConnection(blob->as_block().channel());

  // Destroy the other two VPartitions.
  DestroyPartition(kTestPartDataName);
  CheckWriteReadBlock(sys->as_block(), 0, 1);
  CheckDeadConnection(data->as_block().channel());

  DestroyPartition(kTestPartSystemName);
  CheckDeadConnection(sys->as_block().channel());

  FVMCheckSliceSize(kSliceSize);
}

TEST_F(FvmTest, TestVPartitionQuery) {
  constexpr uint64_t kBlockSize = 512;
  constexpr uint64_t kBlockCount = 1 << 16;
  constexpr uint64_t kSliceSize = 64 * kBlockSize;
  CreateFVM(kBlockSize, kBlockCount, kSliceSize);

  // Allocate partition
  zx::result part_or = AllocatePartition({
      .slice_count = 10,
      .type = kTestPartDataGuid,
      .guid = kTestUniqueGuid1,
      .name = kTestPartDataName,
  });
  ASSERT_OK(part_or);
  std::unique_ptr<fvm::BlockConnector> part = *std::move(part_or);

  // Create non-contiguous extent.
  uint64_t offset = 20;
  uint64_t length = 10;
  {
    const fidl::WireResult result = fidl::WireCall(part->as_block())->Extend(offset, length);
    ASSERT_OK(result.status());
    const fidl::WireResponse response = result.value();
    ASSERT_OK(response.status);
  }

  VolumeManagerInfo info = GetFvmInfo();

  // Query various vslice ranges
  uint64_t start_slices[6];
  start_slices[0] = 0;
  start_slices[1] = 10;
  start_slices[2] = 20;
  start_slices[3] = 50;
  start_slices[4] = 25;
  start_slices[5] = 15;

  // Check response from partition query
  {
    const fidl::WireResult result =
        fidl::WireCall(part->as_block())
            ->QuerySlices(fidl::VectorView<uint64_t>::FromExternal(start_slices));
    ASSERT_OK(result.status());
    const fidl::WireResponse response = result.value();
    ASSERT_OK(response.status);
    fidl::Array ranges = response.response;

    ASSERT_EQ(response.response_count, std::size(start_slices));
    ASSERT_TRUE(ranges[0].allocated);
    ASSERT_EQ(ranges[0].count, 10);
    ASSERT_FALSE(ranges[1].allocated);
    ASSERT_EQ(ranges[1].count, 10);
    ASSERT_TRUE(ranges[2].allocated);
    ASSERT_EQ(ranges[2].count, 10);
    ASSERT_FALSE(ranges[3].allocated);
    ASSERT_EQ(ranges[3].count, info.max_virtual_slice - 50);
    ASSERT_TRUE(ranges[4].allocated);
    ASSERT_EQ(ranges[4].count, 5);
    ASSERT_FALSE(ranges[5].allocated);
    ASSERT_EQ(ranges[5].count, 5);
  }

  // Merge the extents!
  offset = 10;
  length = 10;
  {
    const fidl::WireResult result = fidl::WireCall(part->as_block())->Extend(offset, length);
    ASSERT_OK(result.status());
    const fidl::WireResponse response = result.value();
    ASSERT_OK(response.status);
  }

  // Check partition query response again after extend
  {
    const fidl::WireResult result =
        fidl::WireCall(part->as_block())
            ->QuerySlices(fidl::VectorView<uint64_t>::FromExternal(start_slices));
    ASSERT_OK(result.status());
    const fidl::WireResponse response = result.value();
    ASSERT_OK(response.status);
    fidl::Array ranges = response.response;

    ASSERT_EQ(response.response_count, std::size(start_slices));
    ASSERT_TRUE(ranges[0].allocated);
    ASSERT_EQ(ranges[0].count, 30);
    ASSERT_TRUE(ranges[1].allocated);
    ASSERT_EQ(ranges[1].count, 20);
    ASSERT_TRUE(ranges[2].allocated);
    ASSERT_EQ(ranges[2].count, 10);
    ASSERT_FALSE(ranges[3].allocated);
    ASSERT_EQ(ranges[3].count, info.max_virtual_slice - 50);
    ASSERT_TRUE(ranges[4].allocated);
    ASSERT_EQ(ranges[4].count, 5);
    ASSERT_TRUE(ranges[5].allocated);
    ASSERT_EQ(ranges[5].count, 15);
  }

  start_slices[0] = info.max_virtual_slice + 1;
  const fidl::WireResult result =
      fidl::WireCall(part->as_block())
          ->QuerySlices(fidl::VectorView<uint64_t>::FromExternal(start_slices));
  ASSERT_OK(result.status());
  const fidl::WireResponse response = result.value();
  ASSERT_STATUS(response.status, ZX_ERR_OUT_OF_RANGE);

  FVMCheckSliceSize(kSliceSize);
}

// Test allocating and accessing slices which are allocated contiguously.
TEST_F(FvmTest, TestSliceAccessContiguous) {
  constexpr uint64_t kBlockSize = 512;
  constexpr uint64_t kBlockCount = 1 << 16;
  constexpr uint64_t kSliceSize = 64 * kBlockSize;
  CreateFVM(kBlockSize, kBlockCount, kSliceSize);

  VolumeManagerInfo info = GetFvmInfo();
  size_t slice_size = info.slice_size;

  // Allocate one VPart
  zx::result vp_or = AllocatePartition({
      .type = kTestPartDataGuid,
      .guid = kTestUniqueGuid1,
      .name = kTestPartDataName,
  });
  ASSERT_OK(vp_or);
  std::unique_ptr<fvm::BlockConnector> vp = *std::move(vp_or);
  fidl::ClientEnd device = vp->connect_block();

  const fidl::WireResult result = fidl::WireCall(device)->GetInfo();
  ASSERT_OK(result.status());
  const fit::result response = result.value();
  ASSERT_TRUE(response.is_ok(), "%s", zx_status_get_string(response.error_value()));
  const fuchsia_storage_block::wire::BlockInfo& block_info = response.value()->info;

  // This is the last 'accessible' block.
  size_t last_block = (slice_size / block_info.block_size) - 1;

  {
    auto vc = fbl::MakeRefCounted<VmoClient>(vp->connect_block());
    VmoBuf vb(vc, block_info.block_size * static_cast<size_t>(2));
    vc->CheckWrite(vb, 0, block_info.block_size * last_block, block_info.block_size);
    vc->CheckRead(vb, 0, block_info.block_size * last_block, block_info.block_size);

    // Try writing out of bounds -- check that we don't have access.
    CheckNoAccessBlock(device, (slice_size / block_info.block_size) - 1, 2);
    CheckNoAccessBlock(device, slice_size / block_info.block_size, 1);

    // Attempt to access the next contiguous slice
    {
      const fidl::WireResult result = fidl::WireCall(vp->as_block())->Extend(1, 1);
      ASSERT_OK(result.status());
      const fidl::WireResponse response = result.value();
      ASSERT_OK(response.status);
    }

    // Now we can access the next slice...
    vc->CheckWrite(vb, block_info.block_size, block_info.block_size * (last_block + 1),
                   block_info.block_size);
    vc->CheckRead(vb, block_info.block_size, block_info.block_size * (last_block + 1),
                  block_info.block_size);
    // ... We can still access the previous slice...
    vc->CheckRead(vb, 0, block_info.block_size * last_block, block_info.block_size);
    // ... And we can cross slices
    vc->CheckRead(vb, 0, block_info.block_size * last_block,
                  block_info.block_size * static_cast<size_t>(2));
  }

  vp = {};
  FVMCheckSliceSize(kSliceSize);
}

// Test allocating and accessing multiple (3+) slices at once.
TEST_F(FvmTest, TestSliceAccessMany) {
  // The size of a slice must be carefully constructed for this test
  // so that we can hold multiple slices in memory without worrying
  // about hitting resource limits.
  constexpr uint64_t kBlockSize = 512;
  constexpr uint64_t kBlockCount = 1 << 20;
  constexpr uint64_t kBlocksPerSlice = 256;
  constexpr uint64_t kSliceSize = kBlocksPerSlice * kBlockSize;
  CreateFVM(kBlockSize, kBlockCount, kSliceSize);

  VolumeManagerInfo info = GetFvmInfo();
  ASSERT_EQ(info.slice_size, kSliceSize);

  // Allocate one VPart
  auto vp_or = AllocatePartition({
      .type = kTestPartDataGuid,
      .guid = kTestUniqueGuid1,
      .name = kTestPartDataName,
  });
  ASSERT_OK(vp_or);
  std::unique_ptr<fvm::BlockConnector> vp = *std::move(vp_or);

  fidl::ClientEnd device = vp->connect_block();

  const fidl::WireResult result = fidl::WireCall(device)->GetInfo();
  ASSERT_OK(result.status());
  const fit::result response = result.value();
  ASSERT_TRUE(response.is_ok(), "%s", zx_status_get_string(response.error_value()));
  const fuchsia_storage_block::wire::BlockInfo& block_info = response.value()->info;
  ASSERT_EQ(block_info.block_size, kBlockSize);

  {
    auto vc = fbl::MakeRefCounted<VmoClient>(vp->connect_block());
    VmoBuf vb(vc, kSliceSize * 3);

    // Access the first slice
    vc->CheckWrite(vb, 0, 0, kSliceSize);
    vc->CheckRead(vb, 0, 0, kSliceSize);

    // Try writing out of bounds -- check that we don't have access.
    CheckNoAccessBlock(device, kBlocksPerSlice - 1, 2);
    CheckNoAccessBlock(device, kBlocksPerSlice, 1);

    // Attempt to access the next contiguous slices
    uint64_t offset = 1;
    uint64_t length = 2;
    {
      const fidl::WireResult result = fidl::WireCall(vp->as_block())->Extend(offset, length);
      ASSERT_OK(result.status());
      const fidl::WireResponse response = result.value();
      ASSERT_OK(response.status);
    }

    // Now we can access the next slices...
    vc->CheckWrite(vb, kSliceSize, kSliceSize, 2 * kSliceSize);
    vc->CheckRead(vb, kSliceSize, kSliceSize, 2 * kSliceSize);
    // ... We can still access the previous slice...
    vc->CheckRead(vb, 0, 0, kSliceSize);
    // ... And we can cross slices for reading.
    vc->CheckRead(vb, 0, 0, 3 * kSliceSize);

    // Also, we can cross slices for writing.
    vc->CheckWrite(vb, 0, 0, 3 * kSliceSize);
    vc->CheckRead(vb, 0, 0, 3 * kSliceSize);

    // Additionally, we can access "parts" of slices in a multi-slice
    // operation. Here, read one block into the first slice, and read
    // up to the last block in the final slice.
    vc->CheckWrite(vb, 0, kBlockSize, 3 * kSliceSize - 2 * kBlockSize);
    vc->CheckRead(vb, 0, kBlockSize, 3 * kSliceSize - 2 * kBlockSize);
  }

  vp = {};
  FVMCheckSliceSize(kSliceSize);
  ValidateFVM(ramdisk_block_interface());
}

// Test allocating and accessing slices which are allocated
// virtually contiguously (they appear sequential to the client) but are
// actually noncontiguous on the FVM partition.
TEST_F(FvmTest, TestSliceAccessNonContiguousPhysical) {
  constexpr uint64_t kBlockSize{512};
  constexpr uint64_t kBlockCount{1 << 16};
  constexpr uint64_t kSliceSize{kBlockSize * 64};
  constexpr uint64_t kDiskSize{kBlockSize * kBlockCount};
  CreateFVM(kBlockSize, kBlockCount, kSliceSize);

  GetFvmInfo();

  constexpr size_t kNumVParts = 3;
  constexpr size_t kSliceCount = 1;
  typedef struct vdata {
    std::unique_ptr<fvm::BlockConnector> partition;
    const uuid::Uuid& guid;
    const std::string_view& name;
    size_t slices_used;
  } vdata_t;

  vdata_t vparts[kNumVParts] = {
      {{}, kTestPartDataGuid, kTestPartDataName, kSliceCount},
      {{}, kTestPartBlobGuid, kTestPartBlobName, kSliceCount},
      {{}, kTestPartSystemGuid, kTestPartSystemName, kSliceCount},
  };

  for (auto& vpart : vparts) {
    zx::result part_or = AllocatePartition({
        .slice_count = kSliceCount,
        .type = vpart.guid,
        .guid = kTestUniqueGuid1,
        .name = vpart.name,
    });
    ASSERT_OK(part_or);
    vpart.partition = *std::move(part_or);
  }

  const fidl::WireResult result = fidl::WireCall(vparts[0].partition->as_block())->GetInfo();
  ASSERT_OK(result.status());
  const fit::result response = result.value();
  ASSERT_TRUE(response.is_ok(), "%s", zx_status_get_string(response.error_value()));
  const fuchsia_storage_block::wire::BlockInfo& block_info = response.value()->info;

  size_t usable_slices_per_vpart = UsableSlicesCount(kDiskSize, kSliceSize) / kNumVParts;
  size_t i = 0;
  while (vparts[i].slices_used < usable_slices_per_vpart) {
    // This is the last 'accessible' block.
    size_t last_block = (vparts[i].slices_used * (kSliceSize / block_info.block_size)) - 1;

    auto vc = fbl::MakeRefCounted<VmoClient>(vparts[i].partition->connect_block());
    VmoBuf vb(vc, block_info.block_size * static_cast<size_t>(2));

    vc->CheckWrite(vb, 0, block_info.block_size * last_block, block_info.block_size);
    vc->CheckRead(vb, 0, block_info.block_size * last_block, block_info.block_size);

    // Try writing out of bounds -- check that we don't have access.
    CheckNoAccessBlock(vparts[i].partition->as_block(), last_block, 2);
    CheckNoAccessBlock(vparts[i].partition->as_block(), last_block + 1, 1);

    // Attempt to access the next contiguous slice
    uint64_t offset = vparts[i].slices_used;
    uint64_t length = 1;
    {
      const fidl::WireResult result =
          fidl::WireCall(vparts[i].partition->as_block())->Extend(offset, length);
      ASSERT_OK(result.status());
      const fidl::WireResponse response = result.value();
      ASSERT_OK(response.status);
    }

    // Now we can access the next slice...
    vc->CheckWrite(vb, block_info.block_size, block_info.block_size * (last_block + 1),
                   block_info.block_size);
    vc->CheckRead(vb, block_info.block_size, block_info.block_size * (last_block + 1),
                  block_info.block_size);
    // ... We can still access the previous slice...
    vc->CheckRead(vb, 0, block_info.block_size * last_block, block_info.block_size);
    // ... And we can cross slices
    vc->CheckRead(vb, 0, block_info.block_size * last_block,
                  block_info.block_size * static_cast<size_t>(2));

    vparts[i].slices_used++;
    i = (i + 1) % kNumVParts;
  }

  for (size_t i = 0; i < kNumVParts; i++) {
    printf("Testing multi-slice operations on vslice %lu\n", i);

    // We need at least five slices, so we can access three "middle"
    // slices and jitter to test off-by-one errors.
    ASSERT_GE(vparts[i].slices_used, 5);

    {
      auto vc = fbl::MakeRefCounted<VmoClient>(vparts[i].partition->connect_block());
      VmoBuf vb(vc, kSliceSize * 4);

      // Try accessing 3 noncontiguous slices at once, with the
      // addition of "off by one block".
      size_t dev_off_start = kSliceSize - block_info.block_size;
      size_t dev_off_end = kSliceSize + block_info.block_size;
      size_t len_start = kSliceSize * 3 - block_info.block_size;
      size_t len_end = kSliceSize * 3 + block_info.block_size;

      // Test a variety of:
      // Starting device offsets,
      size_t bsz = block_info.block_size;
      for (size_t dev_off = dev_off_start; dev_off <= dev_off_end; dev_off += bsz) {
        printf("  Testing non-contiguous write/read starting at offset: %zu\n", dev_off);
        // Operation lengths,
        for (size_t len = len_start; len <= len_end; len += bsz) {
          printf("    Testing operation of length: %zu\n", len);
          // and starting VMO offsets
          for (size_t vmo_off = 0; vmo_off < 3 * bsz; vmo_off += bsz) {
            // Try writing & reading the entire section (multiple
            // slices) at once.
            vc->CheckWrite(vb, vmo_off, dev_off, len);
            vc->CheckRead(vb, vmo_off, dev_off, len);

            // Try reading the section one slice at a time.
            // The results should be the same.
            size_t sub_off = 0;
            size_t sub_len = kSliceSize - (dev_off % kSliceSize);
            while (sub_off < len) {
              vc->CheckRead(vb, vmo_off + sub_off, dev_off + sub_off, sub_len);
              sub_off += sub_len;
              sub_len = std::min(kSliceSize, len - sub_off);
            }
          }
        }
      }
    }
    vparts[i].partition = {};
  }

  FVMCheckSliceSize(kSliceSize);
  ValidateFVM(ramdisk_block_interface().borrow());
}

// Test allocating and accessing slices which are
// allocated noncontiguously from the client's perspective.
TEST_F(FvmTest, TestSliceAccessNonContiguousVirtual) {
  constexpr uint64_t kBlockSize{512};
  constexpr uint64_t kBlockCount{1 << 20};
  constexpr uint64_t kSliceSize{UINT64_C(64) * (1 << 20)};
  constexpr uint64_t kDiskSize{kBlockSize * kBlockCount};
  CreateFVM(kBlockSize, kBlockCount, kSliceSize);

  GetFvmInfo();

  constexpr size_t kNumVParts = 3;
  constexpr size_t kSliceCount = 1;
  typedef struct vdata {
    std::unique_ptr<fvm::BlockConnector> partition;
    const uuid::Uuid& guid;
    const std::string_view& name;
    size_t slices_used;
    size_t last_slice;
  } vdata_t;

  vdata_t vparts[kNumVParts] = {
      {{}, kTestPartDataGuid, kTestPartDataName, kSliceCount, kSliceCount},
      {{}, kTestPartBlobGuid, kTestPartBlobName, kSliceCount, kSliceCount},
      {{}, kTestPartSystemGuid, kTestPartSystemName, kSliceCount, kSliceCount},
  };

  for (auto& vpart : vparts) {
    zx::result part_or = AllocatePartition({
        .slice_count = kSliceCount,
        .type = vpart.guid,
        .guid = kTestUniqueGuid1,
        .name = vpart.name,
    });
    ASSERT_OK(part_or);
    vpart.partition = *std::move(part_or);
  }

  const fidl::WireResult result = fidl::WireCall(vparts[0].partition->as_block())->GetInfo();
  ASSERT_OK(result.status());
  const fit::result response = result.value();
  ASSERT_TRUE(response.is_ok(), "%s", zx_status_get_string(response.error_value()));
  const fuchsia_storage_block::wire::BlockInfo& block_info = response.value()->info;

  size_t usable_slices_per_vpart = UsableSlicesCount(kDiskSize, kSliceSize) / kNumVParts;
  size_t i = 0;
  while (vparts[i].slices_used < usable_slices_per_vpart) {
    fidl::UnownedClientEnd device = vparts[i].partition->as_block();
    // This is the last 'accessible' block.
    size_t last_block = (vparts[i].last_slice * (kSliceSize / block_info.block_size)) - 1;
    CheckWriteReadBlock(device, last_block, 1);

    // Try writing out of bounds -- check that we don't have access.
    CheckNoAccessBlock(device, last_block, 2);
    CheckNoAccessBlock(device, last_block + 1, 1);

    // Attempt to access a non-contiguous slice
    uint64_t offset = vparts[i].last_slice + 2;
    uint64_t length = 1;
    {
      const fidl::WireResult result =
          fidl::WireCall(vparts[i].partition->as_block())->Extend(offset, length);
      ASSERT_OK(result.status());
      const fidl::WireResponse response = result.value();
      ASSERT_OK(response.status);
    }

    // We still don't have access to the next slice...
    CheckNoAccessBlock(device, last_block, 2);
    CheckNoAccessBlock(device, last_block + 1, 1);

    // But we have access to the slice we asked for!
    size_t requested_block = (offset * kSliceSize) / block_info.block_size;
    CheckWriteReadBlock(device, requested_block, 1);

    vparts[i].slices_used++;
    vparts[i].last_slice = offset;
    i = (i + 1) % kNumVParts;
  }

  for (vdata_t& vpart : vparts) {
    vpart.partition = {};
  }

  FVMCheckSliceSize(kSliceSize);
  ValidateFVM(ramdisk_block_interface().borrow());
}

// Test that the FVM driver actually persists updates.
TEST_F(FvmTest, TestPersistenceSimple) {
  constexpr uint64_t kBlockSize{512};
  constexpr uint64_t kBlockCount{1 << 20};
  constexpr uint64_t kSliceSize{UINT64_C(64) * (1 << 20)};
  CreateFVM(kBlockSize, kBlockCount, kSliceSize);

  constexpr uint64_t kDiskSize = kBlockSize * kBlockCount;
  size_t slices_left = UsableSlicesCount(kDiskSize, kSliceSize);
  const uint64_t kSliceCount = slices_left;

  GetFvmInfo();

  // Allocate one VPart
  auto vp_or = AllocatePartition({
      .type = kTestPartDataGuid,
      .guid = kTestUniqueGuid1,
      .name = kTestPartDataName,
  });
  ASSERT_OK(vp_or);
  std::unique_ptr<fvm::BlockConnector> vp = *std::move(vp_or);
  slices_left--;

  fidl::UnownedClientEnd device = vp->as_block();

  // Check that the name matches what we provided
  {
    const fidl::WireResult result = fidl::WireCall(vp->as_block())->GetName();
    ASSERT_OK(result.status());
    const fidl::WireResponse response = result.value();
    ASSERT_OK(response.status);
    ASSERT_STREQ(response.name.get(), kTestPartDataName.data());
  }

  fuchsia_storage_block::wire::BlockInfo block_info;
  {
    const fidl::WireResult result = fidl::WireCall(device)->GetInfo();
    ASSERT_OK(result.status());
    const fit::result response = result.value();
    ASSERT_TRUE(response.is_ok(), "%s", zx_status_get_string(response.error_value()));
    block_info = response.value()->info;
  }
  std::unique_ptr<uint8_t[]> buf(new uint8_t[block_info.block_size * static_cast<size_t>(2)]);

  // Check that we can read from / write to it
  CheckWrite(device, 0, block_info.block_size, buf.get());
  CheckRead(device, 0, block_info.block_size, buf.get());
  vp = {};

  // Check that it still exists after rebinding the driver
  FVMRebind();

  vp_or = WaitForPartition(kTestPartDataName);
  ASSERT_OK(vp_or);
  vp = *std::move(vp_or);
  device = vp->as_block();

  CheckRead(device, 0, block_info.block_size, buf.get());

  // Try extending the vpartition, and checking that the extension persists.
  // This is the last 'accessible' block.
  size_t last_block = (kSliceSize / block_info.block_size) - 1;
  CheckWrite(device, block_info.block_size * last_block, block_info.block_size, buf.get());
  CheckRead(device, block_info.block_size * last_block, block_info.block_size, buf.get());

  // Try writing out of bounds -- check that we don't have access.
  CheckNoAccessBlock(vp->as_block(), (kSliceSize / block_info.block_size) - 1, 2);
  CheckNoAccessBlock(vp->as_block(), kSliceSize / block_info.block_size, 1);

  uint64_t offset = 1;
  uint64_t length = 1;
  {
    const fidl::WireResult result = fidl::WireCall(vp->as_block())->Extend(offset, length);
    ASSERT_OK(result.status());
    const fidl::WireResponse response = result.value();
    ASSERT_OK(response.status);
  }
  slices_left--;

  vp = {};
  // FVMRebind will cause the rebind on ramdisk block device. The fvm device is child device
  // to ramdisk block device. Before issuing rebind make sure the fd is released.
  // Rebind the FVM driver, check the extension has succeeded.
  FVMRebind();

  vp_or = WaitForPartition(kTestPartDataName);
  ASSERT_OK(vp_or);
  vp = *std::move(vp_or);
  device = vp->as_block();

  // Now we can access the next slice...
  CheckWrite(device, block_info.block_size * (last_block + 1), block_info.block_size,
             &buf[block_info.block_size]);
  CheckRead(device, block_info.block_size * (last_block + 1), block_info.block_size,
            &buf[block_info.block_size]);
  // ... We can still access the previous slice...
  CheckRead(device, block_info.block_size * last_block, block_info.block_size, buf.get());
  // ... And we can cross slices
  CheckRead(device, block_info.block_size * last_block,
            block_info.block_size * static_cast<size_t>(2), buf.get());

  // Try allocating the rest of the slices, rebinding, and ensuring
  // that the size stays updated.
  {
    const fidl::WireResult result = fidl::WireCall(device)->GetInfo();
    ASSERT_OK(result.status());
    const fit::result response = result.value();
    ASSERT_TRUE(response.is_ok(), "%s", zx_status_get_string(response.error_value()));
    block_info = response.value()->info;
  }
  ASSERT_EQ(block_info.block_count * block_info.block_size, kSliceSize * 2);

  offset = 2;
  length = slices_left;
  {
    const fidl::WireResult result = fidl::WireCall(vp->as_block())->Extend(offset, length);
    ASSERT_OK(result.status());
    const fidl::WireResponse response = result.value();
    ASSERT_OK(response.status);
  }
  {
    const fidl::WireResult result = fidl::WireCall(device)->GetInfo();
    ASSERT_OK(result.status());
    const fit::result response = result.value();
    ASSERT_TRUE(response.is_ok(), "%s", zx_status_get_string(response.error_value()));
    block_info = response.value()->info;
  }
  ASSERT_EQ(block_info.block_count * block_info.block_size, kSliceSize * kSliceCount);

  vp = {};
  FVMRebind();

  vp_or = WaitForPartition(kTestPartDataName);
  ASSERT_OK(vp_or);
  vp = *std::move(vp_or);
  device = vp->as_block();

  {
    const fidl::WireResult result = fidl::WireCall(device)->GetInfo();
    ASSERT_OK(result.status());
    const fit::result response = result.value();
    ASSERT_TRUE(response.is_ok(), "%s", zx_status_get_string(response.error_value()));
    block_info = response.value()->info;
  }
  ASSERT_EQ(block_info.block_count * block_info.block_size, kSliceSize * kSliceCount);

  vp = {};
  FVMCheckSliceSize(64lu * (1 << 20));
}

void CorruptMountHelper(const fvm::BlockConnector& connector, fs_management::DiskFormat disk_format,
                        const size_t* vslice_start, size_t vslice_count) {
  auto component = fs_management::FsComponent::FromDiskFormat(disk_format);
  // Format the VPart as |disk_format|.
  ASSERT_OK(fs_management::Mkfs(connector.connect_block(), component, {}));

  fuchsia_storage_block::wire::VsliceRange
      initial_ranges[fuchsia_storage_block::wire::kMaxSliceRequests];

  // Check initial slice allocation.
  {
    const fidl::WireResult result = fidl::WireCall(connector.as_block())
                                        ->QuerySlices(fidl::VectorView<uint64_t>::FromExternal(
                                            const_cast<size_t*>(vslice_start), vslice_count));
    ASSERT_OK(result.status());
    const fidl::WireResponse response = result.value();
    ASSERT_OK(response.status);
    ASSERT_EQ(vslice_count, response.response_count);

    for (unsigned i = 0; i < response.response_count; i++) {
      ASSERT_TRUE(response.response[i].allocated);
      ASSERT_GT(response.response[i].count, 0);
      initial_ranges[i] = response.response[i];
    }

    // Manually shrink slices so FVM will differ from the partition.
    uint64_t offset = vslice_start[0] + response.response[0].count - 1;
    uint64_t length = 1;
    {
      const fidl::WireResult result = fidl::WireCall(connector.as_block())->Shrink(offset, length);
      ASSERT_OK(result.status());
      const fidl::WireResponse response = result.value();
      ASSERT_OK(response.status);
    }

    // Check slice allocation after manual grow/shrink
    {
      const fidl::WireResult result = fidl::WireCall(connector.as_block())
                                          ->QuerySlices(fidl::VectorView<uint64_t>::FromExternal(
                                              const_cast<size_t*>(vslice_start), vslice_count));
      ASSERT_OK(result.status());
      const fidl::WireResponse response = result.value();
      ASSERT_OK(response.status);
      ASSERT_EQ(vslice_count, response.response_count);
      ASSERT_FALSE(response.response[0].allocated);
      ASSERT_EQ(response.response[0].count, vslice_start[1] - vslice_start[0]);
    }

    // Try to mount the VPart. Since this mount call is supposed to fail, we wait for the spawned
    // fs process to finish and associated fidl channels to close before continuing to try and
    // prevent race conditions with the later mount call.
    ASSERT_NOT_OK(fs_management::Mount(connector.connect_block(), component, {}));

    // We can't reuse the component.
    component = fs_management::FsComponent::FromDiskFormat(disk_format);
  }

  {
    // Grow back the slice we shrunk earlier.
    uint64_t offset = vslice_start[0];
    uint64_t length = 1;
    {
      const fidl::WireResult result = fidl::WireCall(connector.as_block())->Extend(offset, length);
      ASSERT_OK(result.status());
      const fidl::WireResponse response = result.value();
      ASSERT_OK(response.status);
    }

    // Verify grow was successful.
    const fidl::WireResult result = fidl::WireCall(connector.as_block())
                                        ->QuerySlices(fidl::VectorView<uint64_t>::FromExternal(
                                            const_cast<size_t*>(vslice_start), vslice_count));
    ASSERT_OK(result.status());
    const fidl::WireResponse response = result.value();
    ASSERT_OK(response.status);
    ASSERT_EQ(vslice_count, response.response_count);
    ASSERT_TRUE(response.response[0].allocated);
    ASSERT_EQ(response.response[0].count, 1);

    // Now extend all extents by some number of additional slices.
    fuchsia_storage_block::wire::VsliceRange
        ranges_before_extend[fuchsia_storage_block::wire::kMaxSliceRequests];
    for (unsigned i = 0; i < vslice_count; i++) {
      ranges_before_extend[i] = response.response[i];
      uint64_t offset = vslice_start[i] + response.response[i].count;
      uint64_t length = vslice_count - i;
      {
        const fidl::WireResult result =
            fidl::WireCall(connector.as_block())->Extend(offset, length);
        ASSERT_OK(result.status());
        const fidl::WireResponse response = result.value();
        ASSERT_OK(response.status);
      }
    }

    // Verify that the extensions were successful.
    {
      const fidl::WireResult result = fidl::WireCall(connector.as_block())
                                          ->QuerySlices(fidl::VectorView<uint64_t>::FromExternal(
                                              const_cast<size_t*>(vslice_start), vslice_count));
      ASSERT_OK(result.status());
      const fidl::WireResponse response = result.value();
      ASSERT_OK(response.status);
      ASSERT_EQ(vslice_count, response.response_count);
      for (unsigned i = 0; i < vslice_count; i++) {
        ASSERT_TRUE(response.response[i].allocated);
        ASSERT_EQ(response.response[i].count, ranges_before_extend[i].count + vslice_count - i);
      }
    }

    // Try mount again.
    ASSERT_OK(fs_management::Mount(connector.connect_block(), component, {}));
  }

  // Verify that slices were fixed on mount.
  const fidl::WireResult result = fidl::WireCall(connector.as_block())
                                      ->QuerySlices(fidl::VectorView<uint64_t>::FromExternal(
                                          const_cast<size_t*>(vslice_start), vslice_count));
  ASSERT_OK(result.status());
  const fidl::WireResponse response = result.value();
  ASSERT_OK(response.status);
  ASSERT_EQ(vslice_count, response.response_count);

  for (unsigned i = 0; i < vslice_count; i++) {
    ASSERT_TRUE(response.response[i].allocated);
    ASSERT_EQ(response.response[i].count, initial_ranges[i].count);
  }
}

TEST_F(FvmTest, TestCorruptMount) {
  constexpr uint64_t kBlockSize = 512;
  constexpr uint64_t kBlockCount = 1 << 16;
  constexpr uint64_t kSliceSize = 64 * kBlockSize;
  CreateFVM(kBlockSize, kBlockCount, kSliceSize);

  VolumeManagerInfo info = GetFvmInfo();
  ASSERT_EQ(info.slice_size, kSliceSize);

  // Allocate one VPart
  zx::result vp = AllocatePartition({
      .type = kTestPartDataGuid,
      .guid = kTestUniqueGuid1,
      .name = kTestPartDataName,
  });
  ASSERT_OK(vp);

  size_t kMinfsBlocksPerSlice = kSliceSize / minfs::kMinfsBlockSize;
  size_t minfs_vslice_count = 4;
  size_t minfs_vslice_start[] = {
      minfs::kFVMBlockInodeBmStart / kMinfsBlocksPerSlice,
      minfs::kFVMBlockDataBmStart / kMinfsBlocksPerSlice,
      minfs::kFVMBlockInodeStart / kMinfsBlocksPerSlice,
      minfs::kFVMBlockDataStart / kMinfsBlocksPerSlice,
  };

  // Run the test for Minfs.
  CorruptMountHelper(**vp, fs_management::kDiskFormatMinfs, minfs_vslice_start, minfs_vslice_count);

  size_t kBlobfsBlocksPerSlice = kSliceSize / blobfs::kBlobfsBlockSize;
  size_t blobfs_vslice_count = 3;
  size_t blobfs_vslice_start[] = {
      blobfs::kFVMBlockMapStart / kBlobfsBlocksPerSlice,
      blobfs::kFVMNodeMapStart / kBlobfsBlocksPerSlice,
      blobfs::kFVMDataStart / kBlobfsBlocksPerSlice,
  };

  // Run the test for Blobfs.
  CorruptMountHelper(**vp, fs_management::kDiskFormatBlobfs, blobfs_vslice_start,
                     blobfs_vslice_count);
}

// Test that the FVM driver can mount filesystems.
TEST_F(FvmTest, TestMounting) {
  constexpr uint64_t kBlockSize = 512;
  constexpr uint64_t kBlockCount = 1 << 16;
  constexpr uint64_t kSliceSize = 64 * kBlockSize;
  CreateFVM(kBlockSize, kBlockCount, kSliceSize);

  GetFvmInfo();

  // Allocate one VPart
  size_t slice_count = 5;
  zx::result vp_or = AllocatePartition({
      .slice_count = slice_count,
      .type = kTestPartDataGuid,
      .guid = kTestUniqueGuid1,
      .name = kTestPartDataName,
  });
  ASSERT_OK(vp_or);
  std::unique_ptr<fvm::BlockConnector> vp(*std::move(vp_or));

  // Format the VPart as minfs
  auto component = fs_management::FsComponent::FromDiskFormat(fs_management::kDiskFormatMinfs);
  ASSERT_OK(fs_management::Mkfs(vp->connect_block(), component, {}));

  // Mount the VPart
  auto mounted_filesystem = fs_management::Mount(vp->connect_block(), component, {});
  ASSERT_OK(mounted_filesystem);
  auto data = mounted_filesystem->DataRoot();
  ASSERT_OK(data);
  auto binding = fs_management::NamespaceBinding::Create(kMountPath, std::move(*data));
  ASSERT_OK(binding);

  // Verify that the mount was successful.
  fbl::unique_fd rootfd;
  ASSERT_OK(fdio_open3_fd(kMountPath, static_cast<uint64_t>(fuchsia_io::Flags::kProtocolDirectory),
                          rootfd.reset_and_get_address()));
  fdio_cpp::FdioCaller caller(std::move(rootfd));
  auto result = fidl::WireCall(caller.directory())->QueryFilesystem();
  ASSERT_TRUE(result.ok());
  const char* kFsName = "minfs";
  const char* name = reinterpret_cast<const char*>(result.value().info->name.data());
  ASSERT_EQ(strncmp(name, kFsName, strlen(kFsName)), 0, "Unexpected filesystem mounted");

  // Verify that MinFS does not try to use more of the VPartition than
  // was originally allocated.
  ASSERT_LE(result.value().info->total_bytes, kSliceSize * slice_count);

  // Clean up.
  FVMCheckSliceSize(kSliceSize);
}

// Test that FVM-aware filesystem can be reformatted.
TEST_F(FvmTest, TestMkfs) {
  constexpr uint64_t kBlockSize = 512;
  constexpr uint64_t kBlockCount = 1 << 16;
  constexpr uint64_t kSliceSize = 64 * kBlockSize;
  CreateFVM(kBlockSize, kBlockCount, kSliceSize);

  GetFvmInfo();

  // Allocate one VPart.
  size_t slice_count = 5;
  zx::result vp_or = AllocatePartition({
      .slice_count = slice_count,
      .type = kTestPartDataGuid,
      .guid = kTestUniqueGuid1,
      .name = kTestPartDataName,
  });
  ASSERT_OK(vp_or);
  std::unique_ptr<fvm::BlockConnector> vp(*std::move(vp_or));

  // Format the VPart as minfs.
  auto minfs_component =
      fs_management::FsComponent::FromDiskFormat(fs_management::kDiskFormatMinfs);
  ASSERT_OK(fs_management::Mkfs(vp->connect_block(), minfs_component, {}));

  // Format it as MinFS again, even though it is already formatted.
  ASSERT_OK(fs_management::Mkfs(vp->connect_block(), minfs_component, {}));

  // Now try reformatting as blobfs.
  auto blobfs_component =
      fs_management::FsComponent::FromDiskFormat(fs_management::kDiskFormatBlobfs);
  ASSERT_OK(fs_management::Mkfs(vp->connect_block(), blobfs_component, {}));

  // Demonstrate that mounting as minfs will fail, but mounting as blobfs
  // is successful.
  ASSERT_NOT_OK(fs_management::Mount(vp->connect_block(), minfs_component, {}));

  // We can't reuse the component.
  minfs_component = fs_management::FsComponent::FromDiskFormat(fs_management::kDiskFormatMinfs);

  ASSERT_OK(fs_management::Mount(vp->connect_block(), blobfs_component, {}));

  // ... and reformat back to MinFS again.
  ASSERT_OK(fs_management::Mkfs(vp->connect_block(), minfs_component, {}));

  // Mount the VPart.
  auto mounted_filesystem = fs_management::Mount(vp->connect_block(), minfs_component, {});
  ASSERT_OK(mounted_filesystem);
  auto data = mounted_filesystem->DataRoot();
  ASSERT_OK(data);
  auto binding = fs_management::NamespaceBinding::Create(kMountPath, std::move(*data));
  ASSERT_OK(binding);

  // Verify that the mount was successful.
  fbl::unique_fd rootfd;
  ASSERT_OK(fdio_open3_fd(kMountPath, static_cast<uint64_t>(fuchsia_io::Flags::kProtocolDirectory),
                          rootfd.reset_and_get_address()));
  ASSERT_TRUE(rootfd);
  fdio_cpp::FdioCaller caller(std::move(rootfd));
  auto result = fidl::WireCall(caller.directory())->QueryFilesystem();
  ASSERT_TRUE(result.ok());
  const char* kFsName = "minfs";
  const char* name = reinterpret_cast<const char*>(result.value().info->name.data());
  ASSERT_EQ(strncmp(name, kFsName, strlen(kFsName)), 0, "Unexpected filesystem mounted");

  // Verify that MinFS does not try to use more of the VPartition than
  // was originally allocated.
  ASSERT_LE(result.value().info->total_bytes, kSliceSize * slice_count);

  // Clean up.
  FVMCheckSliceSize(kSliceSize);
}

// Test that the FVM can recover when one copy of
// metadata becomes corrupt.
TEST_F(FvmTest, TestCorruptionOk) {
  constexpr uint64_t kBlockSize = 512;
  constexpr uint64_t kBlockCount = 1 << 16;
  constexpr uint64_t kSliceSize = 64 * kBlockSize;
  CreateFVM(kBlockSize, kBlockCount, kSliceSize);

  GetFvmInfo();

  // Allocate one VPart (writes to backup)
  zx::result vp_or = AllocatePartition({
      .type = kTestPartDataGuid,
      .guid = kTestUniqueGuid1,
      .name = kTestPartDataName,
  });
  ASSERT_OK(vp_or);
  std::unique_ptr<fvm::BlockConnector> vp(*std::move(vp_or));

  // Extend the vpart (writes to primary)
  uint64_t offset = 1;
  uint64_t length = 1;
  {
    const fidl::WireResult result = fidl::WireCall(vp->as_block())->Extend(offset, length);
    ASSERT_OK(result.status());
    const fidl::WireResponse response = result.value();
    ASSERT_OK(response.status);
  }
  const fidl::WireResult result = fidl::WireCall(vp->as_block())->GetInfo();
  ASSERT_OK(result.status());
  const fit::result response = result.value();
  ASSERT_TRUE(response.is_ok(), "%s", zx_status_get_string(response.error_value()));
  const fuchsia_storage_block::wire::BlockInfo& block_info = response.value()->info;
  ASSERT_EQ(block_info.block_count * block_info.block_size, kSliceSize * 2);

  // Initial slice access
  CheckWriteReadBlock(vp->as_block(), 0, 1);
  // Extended slice access
  CheckWriteReadBlock(vp->as_block(), kSliceSize / block_info.block_size, 1);

  vp = {};

  // Corrupt the (backup) metadata and rebind.
  // The 'primary' was the last one written, so it'll be used.
  fvm::Header header =
      fvm::Header::FromDiskSize(fvm::kMaxUsablePartitions, kBlockSize * kBlockCount, kSliceSize);
  auto off = static_cast<off_t>(header.GetSuperblockOffset(fvm::SuperblockType::kSecondary));
  uint8_t buf[fvm::kBlockSize];
  fidl::ClientEnd device = ramdisk_block_interface();
  ASSERT_OK(block_client::SingleReadBytes(device.borrow(), buf, sizeof(buf), off));
  // Modify an arbitrary byte (not the magic bits; we still want it to mount!)
  buf[128]++;
  ASSERT_OK(block_client::SingleWriteBytes(device.borrow(), buf, sizeof(buf), off));

  FVMRebind();

  vp_or = WaitForPartition(kTestPartDataName);
  ASSERT_OK(vp_or, "Couldn't re-open Data VPart");
  vp = *std::move(vp_or);

  // The slice extension is still accessible.
  CheckWriteReadBlock(vp->as_block(), 0, 1);
  CheckWriteReadBlock(vp->as_block(), kSliceSize / block_info.block_size, 1);

  // Clean up
  vp = {};

  FVMCheckSliceSize(kSliceSize);
}

TEST_F(FvmTest, TestCorruptionRegression) {
  constexpr uint64_t kBlockSize = 512;
  constexpr uint64_t kBlockCount = 1 << 16;
  constexpr uint64_t kSliceSize = 64 * kBlockSize;
  CreateFVM(kBlockSize, kBlockCount, kSliceSize);

  VolumeManagerInfo info = GetFvmInfo();
  size_t slice_size = info.slice_size;

  // Allocate one VPart (writes to backup)
  zx::result vp_or = AllocatePartition({
      .type = kTestPartDataGuid,
      .guid = kTestUniqueGuid1,
      .name = kTestPartDataName,
  });
  ASSERT_OK(vp_or);
  std::unique_ptr<fvm::BlockConnector> vp(*std::move(vp_or));

  // Extend the vpart (writes to primary)
  uint64_t offset = 1;
  uint64_t length = 1;
  {
    const fidl::WireResult result = fidl::WireCall(vp->as_block())->Extend(offset, length);
    ASSERT_OK(result.status());
    const fidl::WireResponse response = result.value();
    ASSERT_OK(response.status);
  }
  const fidl::WireResult result = fidl::WireCall(vp->as_block())->GetInfo();
  ASSERT_OK(result.status());
  const fit::result response = result.value();
  ASSERT_TRUE(response.is_ok(), "%s", zx_status_get_string(response.error_value()));
  const fuchsia_storage_block::wire::BlockInfo& block_info = response.value()->info;
  ASSERT_EQ(block_info.block_count * block_info.block_size, slice_size * 2);

  // Initial slice access
  CheckWriteReadBlock(vp->as_block(), 0, 1);
  // Extended slice access
  CheckWriteReadBlock(vp->as_block(), slice_size / block_info.block_size, 1);

  vp = {};

  // Corrupt the (primary) metadata and rebind.
  // The 'primary' was the last one written, so the backup will be used.
  off_t off = 0;
  uint8_t buf[fvm::kBlockSize];
  fidl::ClientEnd device = ramdisk_block_interface();
  ASSERT_OK(block_client::SingleReadBytes(device.borrow(), buf, sizeof(buf), off));
  buf[128]++;
  ASSERT_OK(block_client::SingleWriteBytes(device.borrow(), buf, sizeof(buf), off));

  FVMRebind();

  vp_or = WaitForPartition(kTestPartDataName);
  ASSERT_OK(vp_or);
  vp = *std::move(vp_or);

  // The slice extension is no longer accessible
  CheckWriteReadBlock(vp->as_block(), 0, 1);
  CheckNoAccessBlock(vp->as_block(), slice_size / block_info.block_size, 1);

  // Clean up
  vp = {};
  FVMCheckSliceSize(kSliceSize);
}

TEST_F(FvmTest, TestCorruptionUnrecoverable) {
  constexpr uint64_t kBlockSize = 512;
  constexpr uint64_t kBlockCount = 1 << 16;
  constexpr uint64_t kSliceSize = 64 * kBlockSize;
  CreateFVM(kBlockSize, kBlockCount, kSliceSize);

  // Allocate one VPart (writes to backup)
  zx::result vp_or = AllocatePartition({
      .type = kTestPartDataGuid,
      .guid = kTestUniqueGuid1,
      .name = kTestPartDataName,
  });
  ASSERT_OK(vp_or);
  std::unique_ptr<fvm::BlockConnector> vp(*std::move(vp_or));

  // Extend the vpart (writes to primary)
  uint64_t offset = 1;
  uint64_t length = 1;
  {
    const fidl::WireResult result = fidl::WireCall(vp->as_block())->Extend(offset, length);
    ASSERT_OK(result.status());
    const fidl::WireResponse response = result.value();
    ASSERT_OK(response.status);
  }
  const fidl::WireResult result = fidl::WireCall(vp->as_block())->GetInfo();
  ASSERT_OK(result.status());
  const fit::result response = result.value();
  ASSERT_TRUE(response.is_ok(), "%s", zx_status_get_string(response.error_value()));
  const fuchsia_storage_block::wire::BlockInfo& block_info = response.value()->info;
  ASSERT_EQ(block_info.block_count * block_info.block_size, kSliceSize * 2);

  // Initial slice access
  CheckWriteReadBlock(vp->as_block(), 0, 1);
  // Extended slice access
  CheckWriteReadBlock(vp->as_block(), kSliceSize / block_info.block_size, 1);

  vp = {};

  // Corrupt both copies of the metadata.
  // The 'primary' was the last one written, so the backup will be used.
  off_t off = 0;
  uint8_t buf[fvm::kBlockSize];
  fidl::ClientEnd device = ramdisk_block_interface();
  ASSERT_OK(block_client::SingleReadBytes(device.borrow(), buf, sizeof(buf), off));
  buf[128]++;
  ASSERT_OK(block_client::SingleWriteBytes(device.borrow(), buf, sizeof(buf), off));

  fvm::Header header =
      fvm::Header::FromDiskSize(fvm::kMaxUsablePartitions, kBlockSize * kBlockCount, kSliceSize);
  off = static_cast<off_t>(header.GetSuperblockOffset(fvm::SuperblockType::kSecondary));
  ASSERT_OK(block_client::SingleReadBytes(device.borrow(), buf, sizeof(buf), off));
  buf[128]++;
  ASSERT_OK(block_client::SingleWriteBytes(device.borrow(), buf, sizeof(buf), off));

  ValidateFVM(ramdisk_block_interface().borrow(), ValidationResult::Corrupted);
}

// Tests the FVM checker against a just-initialized FVM.
TEST_F(FvmTest, TestCheckNewFVM) {
  CreateFVM(512, 1 << 20, 64LU * (1 << 20));
  fidl::ClientEnd device = ramdisk_block_interface();
  const fidl::WireResult result = fidl::WireCall(device.borrow())->GetInfo();
  ASSERT_OK(result.status());
  const fit::result response = result.value();
  ASSERT_TRUE(response.is_ok(), "%s", zx_status_get_string(response.error_value()));
  const fuchsia_storage_block::wire::BlockInfo& block_info = response.value()->info;
  fvm::Checker checker(device.borrow(), block_info.block_size, true);
  ASSERT_TRUE(checker.Validate());
}

TEST_F(FvmTest, TestPreventDuplicateDeviceNames) {
  constexpr uint64_t kBlockSize = 512;
  constexpr uint64_t kBlockCount = 1 << 16;
  constexpr uint64_t kSliceSize = 64 * kBlockSize;
  CreateFVM(kBlockSize, kBlockCount, kSliceSize);

  // When a partition is destroyed, the slot in FVM is synchronously freed but the device is
  // asynchronously removed. DFv2 prevents multiple child devices with the same name from being
  // bound. This test rapidly allocates and destroys the same partition to try and get a race
  // between the new device being bound and the old device being removed to try and get FVM to
  // bind multiple devices with the same name.
  for (int i = 0; i < 10; ++i) {
    zx::result vp_or = AllocatePartition({
        .type = kTestPartDataGuid,
        .guid = kTestUniqueGuid1,
        .name = kTestPartDataName,
    });
    ASSERT_OK(vp_or);
    DestroyPartition(kTestPartDataName);
  }
}

}  // namespace
