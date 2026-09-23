// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.boot.metadata/cpp/fidl.h>
#include <lib/ddk/metadata.h>
#include <lib/driver/compat/cpp/banjo_client.h>
#include <lib/driver/compat/cpp/metadata.h>
#include <lib/driver/metadata/cpp/metadata.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/stdcompat/span.h>
#include <lib/zbi-format/partition.h>
#include <stdio.h>
#include <stdlib.h>
#include <zircon/process.h>

#include <atomic>
#include <memory>

#include <fbl/alloc_checker.h>
#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/devices/nand/drivers/ram-nand/ram-nand-ctl.h"
#include "src/lib/testing/predicates/status.h"

namespace {

constexpr size_t kPageSize = 4096;
constexpr size_t kOobSize = 4;
constexpr size_t kBlockSize = 4;
constexpr size_t kNumBlocks = 5;
constexpr size_t kNumPages = kBlockSize * kNumBlocks;

fuchsia_hardware_nand::wire::RamNandInfo BuildConfig() {
  fuchsia_hardware_nand::wire::RamNandInfo config = {};
  config.nand_info = {.page_size = 4096,
                      .pages_per_block = 4,
                      .num_blocks = 5,
                      .ecc_bits = 6,
                      .oob_size = 0,
                      .nand_class = fuchsia_hardware_nand::wire::Class::kTest,
                      .partition_guid = {}};
  return config;
}

}  // namespace

namespace ram_nand::testing {

class RamNandTestEnvironment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override { return zx::ok(); }
};

class FixtureConfig final {
 public:
  using DriverType = RamNandCtl;
  using EnvironmentType = RamNandTestEnvironment;
};

class RamNandTest : public ::testing::Test {
 public:
  void SetUp() override {
    ASSERT_OK(driver_test_.StartDriver());

    zx::result ram_nand_ctl =
        driver_test_.ConnectThroughDevfs<fuchsia_hardware_nand::RamNandCtl>("nand-ctl");
    ASSERT_OK(ram_nand_ctl);
    ram_nand_ctl_.Bind(std::move(ram_nand_ctl.value()));
  }

  void TearDown() override { ASSERT_OK(driver_test_.StopDriver()); }

 protected:
  std::shared_ptr<fdf::Namespace> CreateFromDriverVfs() {
    std::vector<fuchsia_component_runner::ComponentNamespaceEntry> namespace_entries;
    namespace_entries.emplace_back(fuchsia_component_runner::ComponentNamespaceEntry{
        {.path = "/svc", .directory = driver_test_.ConnectToDriverSvcDir()}});
    zx::result from_driver_vfs = fdf::Namespace::Create(namespace_entries);
    EXPECT_OK(from_driver_vfs);

    return std::make_shared<fdf::Namespace>(std::move(from_driver_vfs.value()));
  }

  std::string CreateDevice(fuchsia_hardware_nand::wire::RamNandInfo nand_info) {
    fidl::WireResult result = ram_nand_ctl_->CreateDevice(std::move(nand_info));
    EXPECT_OK(result.status());
    return std::string(result->name.get());
  }

  // `ddk::NandProtocolClient` must only be used on the driver's dispatcher because it is a banjo
  // protocol.
  void WithNand(std::string_view device_name, fit::callback<void(ddk::NandProtocolClient)> task) {
    zx::result nand =
        compat::ConnectBanjo<ddk::NandProtocolClient>(CreateFromDriverVfs(), device_name);
    ASSERT_OK(nand);

    driver_test_.RunInDriverContext(
        [task = std::move(task), nand = nand.value()](auto& _) mutable { task(nand); });
  }

  fdf_testing::BackgroundDriverTest<FixtureConfig>& driver_test() { return driver_test_; }

 protected:
  fidl::WireSyncClient<fuchsia_hardware_nand::RamNandCtl>& ram_nand_ctl() { return ram_nand_ctl_; }

 private:
  fdf_testing::BackgroundDriverTest<FixtureConfig> driver_test_;
  fidl::WireSyncClient<fuchsia_hardware_nand::RamNandCtl> ram_nand_ctl_;
};

// Verify that the driver uses the correct names for nand devices.
TEST_F(RamNandTest, NandDeviceNames) {
  const std::string device_name_1 = CreateDevice(BuildConfig());
  ASSERT_EQ(device_name_1, "ram-nand-0");
  const std::string device_name_2 = CreateDevice(BuildConfig());
  ASSERT_EQ(device_name_2, "ram-nand-1");
}

TEST_F(RamNandTest, ExportNandConfig) {
  static const std::array<uint8_t, 16> kExtraPartitionConfig1UniqueGuid = {
      11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11};
  static const std::array<uint8_t, 16> kExtraPartitionConfig3UniqueGuid = {
      22, 22, 22, 22, 22, 22, 22, 22, 22, 22, 22, 22, 22, 22, 22, 22};

  fuchsia_hardware_nand::wire::RamNandInfo config = BuildConfig();
  config.export_nand_config = true;
  config.partition_map.partition_count = 3;

  // Setup the first and third partitions with extra copies, and the second one with a bbt.
  std::ranges::copy(kExtraPartitionConfig1UniqueGuid,
                    config.partition_map.partitions[0].unique_guid.begin());
  config.partition_map.partitions[0].copy_count = 12;
  config.partition_map.partitions[0].copy_byte_offset = 13;

  config.partition_map.partitions[1].first_block = 66;
  config.partition_map.partitions[1].last_block = 77;
  config.partition_map.partitions[1].hidden = true;
  config.partition_map.partitions[1].bbt = true;

  std::ranges::copy(kExtraPartitionConfig3UniqueGuid,
                    config.partition_map.partitions[2].unique_guid.begin());
  memset(config.partition_map.partitions[2].unique_guid.data(), 22, ZBI_PARTITION_GUID_LEN);
  config.partition_map.partitions[2].copy_count = 23;
  config.partition_map.partitions[2].copy_byte_offset = 24;

  const std::string device_name = CreateDevice(std::move(config));

  zx::result nand_config = compat::GetMetadata<fuchsia_hardware_nand::Config>(
      CreateFromDriverVfs(), DEVICE_METADATA_PRIVATE, device_name);
  ASSERT_OK(nand_config);
  static const fuchsia_hardware_nand::Config kExpectedNandConfig(
      {.bad_block_config = {{
           .type = fuchsia_hardware_nand::BadBlockConfigType::kAmlogicUboot,
           .table_start_block = 66,
           .table_end_block = 77,
       }},
       .extra_partition_configs = std::vector<fuchsia_hardware_nand::PartitionConfig>{
           {{
               .type_guid = kExtraPartitionConfig1UniqueGuid,
               .copy_count = 12,
               .copy_byte_offset = 13,
           }},
           {{
               .type_guid = kExtraPartitionConfig3UniqueGuid,
               .copy_count = 23,
               .copy_byte_offset = 24,
           }}}});
  EXPECT_EQ(nand_config.value(), kExpectedNandConfig);
}

TEST_F(RamNandTest, ExportPartitionMap) {
  static const std::array<uint8_t, 16> kGuid = {
      13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13,
  };
  static constexpr std::string kPartition1Name = "partition 1";
  static const std::array<uint8_t, 16> kPartition1TypeGuid = {44, 44, 44, 44, 44, 44, 44, 44,
                                                              44, 44, 44, 44, 44, 44, 44, 44};
  static const std::array<uint8_t, 16> kPartition1UniqueGuid = {45, 45, 45, 45, 45, 45, 45, 45,
                                                                45, 45, 45, 45, 45, 45, 45, 45};
  static constexpr std::string kPartition3Name = "partition 1";
  static const std::array<uint8_t, 16> kPartition3TypeGuid = {55, 55, 55, 55, 55, 55, 55, 55,
                                                              55, 55, 55, 55, 55, 55, 55, 55};
  static const std::array<uint8_t, 16> kPartition3UniqueGuid = {56, 56, 56, 56, 56, 56, 56, 56,
                                                                56, 56, 56, 56, 56, 56, 56, 56};

  fuchsia_hardware_nand::wire::RamNandInfo config = BuildConfig();
  {
    config.export_partition_map = true;
    config.partition_map.partition_count = 3;
    std::ranges::copy(kGuid, config.partition_map.device_guid.begin());

    // Setup the first and third partitions with data, and the second one hidden.
    fuchsia_hardware_nand::wire::Partition& partition1 = config.partition_map.partitions[0];
    std::ranges::copy(kPartition1TypeGuid, partition1.type_guid.begin());
    std::ranges::copy(kPartition1UniqueGuid, partition1.unique_guid.begin());
    partition1.first_block = 46;
    partition1.last_block = 47;
    memcpy(partition1.name.data(), kPartition1Name.c_str(), kPartition1Name.size() + 1);

    config.partition_map.partitions[1].hidden = true;

    fuchsia_hardware_nand::wire::Partition& partition3 = config.partition_map.partitions[2];
    std::ranges::copy(kPartition3TypeGuid, partition3.type_guid.begin());
    std::ranges::copy(kPartition3UniqueGuid, partition3.unique_guid.begin());
    partition3.first_block = 57;
    partition3.last_block = 58;
    memcpy(partition3.name.data(), kPartition3Name.c_str(), kPartition3Name.size() + 1);
  }

  const std::string device_name = CreateDevice(std::move(config));

  zx::result partition_map_result = fdf_metadata::GetMetadata<fuchsia_boot_metadata::PartitionMap>(
      driver_test().ConnectToDriverSvcDir(), device_name);
  ASSERT_OK(partition_map_result);
  const fuchsia_boot_metadata::PartitionMap& partition_map = partition_map_result.value();
  ASSERT_TRUE(partition_map.block_count().has_value());
  EXPECT_EQ(partition_map.block_count().value(), kNumBlocks);
  ASSERT_TRUE(partition_map.block_size().has_value());
  EXPECT_EQ(partition_map.block_size().value(), kPageSize * kBlockSize);
  ASSERT_TRUE(partition_map.guid().has_value());
  EXPECT_EQ(partition_map.guid().value(), kGuid);

  ASSERT_TRUE(partition_map.partitions().has_value());
  const std::vector<fuchsia_boot_metadata::Partition>& partitions =
      partition_map.partitions().value();
  ASSERT_EQ(partitions.size(), 2u);

  const fuchsia_boot_metadata::Partition& partition1 = partitions[0];
  EXPECT_EQ(partition1.type_guid(), kPartition1TypeGuid);
  EXPECT_EQ(partition1.unique_guid(), kPartition1UniqueGuid);
  EXPECT_EQ(partition1.first_block(), 46u);
  EXPECT_EQ(partition1.last_block(), 47u);
  EXPECT_EQ(partition1.name(), kPartition1Name);

  const fuchsia_boot_metadata::Partition& partition2 = partitions[1];
  EXPECT_EQ(partition2.type_guid(), kPartition3TypeGuid);
  EXPECT_EQ(partition2.unique_guid(), kPartition3UniqueGuid);
  EXPECT_EQ(partition2.first_block(), 57u);
  EXPECT_EQ(partition2.last_block(), 58u);
  EXPECT_EQ(partition2.name(), kPartition3Name);

  // EXPECT_EQ(partition_map.value(), kExpectedPartitionMap);
}

TEST_F(RamNandTest, ExportPartitionMapCountClamping) {
  fuchsia_hardware_nand::wire::RamNandInfo config = BuildConfig();
  config.export_partition_map = true;
  config.partition_map.partition_count = fuchsia_hardware_nand::wire::kMaxPartitions + 5;

  const std::string device_name = CreateDevice(std::move(config));

  zx::result partition_map_result = fdf_metadata::GetMetadata<fuchsia_boot_metadata::PartitionMap>(
      driver_test().ConnectToDriverSvcDir(), device_name);
  ASSERT_OK(partition_map_result);
  const fuchsia_boot_metadata::PartitionMap& partition_map = partition_map_result.value();
  ASSERT_TRUE(partition_map.partitions().has_value());
  EXPECT_EQ(partition_map.partitions().value().size(),
            static_cast<size_t>(fuchsia_hardware_nand::wire::kMaxPartitions));
}

TEST_F(RamNandTest, AddMetadata) {
  fuchsia_hardware_nand::wire::RamNandInfo config = BuildConfig();
  config.export_nand_config = true;
  config.export_partition_map = true;

  const std::string device_name = CreateDevice(std::move(config));

  auto from_driver_vfs = CreateFromDriverVfs();
  zx::result nand_config = compat::GetMetadata<fuchsia_hardware_nand::Config>(
      from_driver_vfs, DEVICE_METADATA_PRIVATE, device_name);
  ASSERT_OK(nand_config);

  zx::result partition_map = fdf_metadata::GetMetadata<fuchsia_boot_metadata::PartitionMap>(
      driver_test().ConnectToDriverSvcDir(), device_name);
  ASSERT_OK(partition_map);
  EXPECT_TRUE(partition_map.value().partitions().has_value());
  EXPECT_TRUE(partition_map.value().partitions().value().empty());
}

TEST_F(RamNandTest, Unlink) {
  const std::string device_name = CreateDevice(BuildConfig());
  const std::vector<std::string> devfs_node_name_path = {std::string(RamNandCtl::kChildNodeName),
                                                         device_name};
  zx::result client_end =
      driver_test().ConnectThroughDevfs<fuchsia_hardware_nand::RamNand>(devfs_node_name_path);
  ASSERT_OK(client_end);
  fidl::WireSyncClient<fuchsia_hardware_nand::RamNand> client(std::move(client_end.value()));

  {
    const fidl::WireResult result = client->Unlink();
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().status);
  }

  // The device is "dead" now.
  {
    const fidl::WireResult result = client->Unlink();
    ASSERT_EQ(ZX_ERR_PEER_CLOSED, result.status());
  }
}

TEST_F(RamNandTest, Query) {
  const std::string device_name = CreateDevice(BuildConfig());

  WithNand(device_name, [](ddk::NandProtocolClient device) {
    nand_info_t info;
    size_t operation_size;
    device.Query(&info, &operation_size);
    ASSERT_EQ(info.page_size, 4096u);
    ASSERT_EQ(info.pages_per_block, 4u);
    ASSERT_EQ(info.num_blocks, 5u);
    ASSERT_EQ(info.ecc_bits, 6u);
    ASSERT_EQ(info.oob_size, 0u);
  });
}

// Data to be pre-pended to a nand_op_t issued to the device.
struct OpHeader {
  class Operation* operation;
  class NandDeviceTest* test;
};

// Wrapper for a nand_operation_t.
class Operation {
 public:
  explicit Operation(size_t op_size, NandDeviceTest* test = 0)
      : op_size_(op_size + sizeof(OpHeader)), test_(test) {}
  ~Operation() {
    if (mapped_addr_) {
      zx_vmar_unmap(zx_vmar_root_self(), reinterpret_cast<uintptr_t>(mapped_addr_), buffer_size_);
    }
  }

  // Accessors for the memory represented by the operation's vmo.
  size_t buffer_size() { return buffer_size_; }
  char* buffer() const { return mapped_addr_; }

  // Creates a vmo and sets the handle on the nand_operation_t.
  bool SetDataVmo();
  bool SetOobVmo();

  nand_operation_t* GetOperation();

  void OnCompletion(zx_status_t status) {
    status_ = status;
    completed_ = true;
  }

  bool completed() const { return completed_; }
  zx_status_t status() const { return status_; }

 private:
  zx_handle_t GetVmo();
  void CreateOperation();

  zx::vmo vmo_;
  char* mapped_addr_ = nullptr;
  size_t op_size_;
  NandDeviceTest* test_;
  zx_status_t status_ = ZX_ERR_ACCESS_DENIED;
  bool completed_ = false;
  static constexpr size_t buffer_size_ = (kPageSize + kOobSize) * kNumPages;
  std::unique_ptr<char[]> raw_buffer_;
  DISALLOW_COPY_ASSIGN_AND_MOVE(Operation);
};

bool Operation::SetDataVmo() {
  nand_operation_t* operation = GetOperation();
  if (!operation) {
    return false;
  }
  if (operation->command == NAND_OP_READ_BYTES || operation->command == NAND_OP_WRITE_BYTES) {
    operation->rw_bytes.data_vmo = GetVmo();
    operation->rw_bytes.offset_data_vmo = 0;
    return operation->rw_bytes.data_vmo != ZX_HANDLE_INVALID;
  }
  operation->rw.data_vmo = GetVmo();
  operation->rw_bytes.offset_data_vmo = 0;
  return operation->rw.data_vmo != ZX_HANDLE_INVALID;
}

bool Operation::SetOobVmo() {
  nand_operation_t* operation = GetOperation();
  if (!operation) {
    return false;
  }
  operation->rw.oob_vmo = GetVmo();
  return operation->rw.oob_vmo != ZX_HANDLE_INVALID;
}

nand_operation_t* Operation::GetOperation() {
  if (!raw_buffer_) {
    CreateOperation();
  }
  return reinterpret_cast<nand_operation_t*>(raw_buffer_.get() + sizeof(OpHeader));
}

zx_handle_t Operation::GetVmo() {
  if (vmo_.is_valid()) {
    return vmo_.get();
  }

  zx_status_t status = zx::vmo::create(buffer_size_, 0, &vmo_);
  if (status != ZX_OK) {
    return ZX_HANDLE_INVALID;
  }

  uintptr_t address;
  status = zx_vmar_map(zx_vmar_root_self(), ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo_.get(), 0,
                       buffer_size_, &address);
  if (status != ZX_OK) {
    return ZX_HANDLE_INVALID;
  }
  mapped_addr_ = reinterpret_cast<char*>(address);
  return vmo_.get();
}

void Operation::CreateOperation() {
  fbl::AllocChecker checker;
  raw_buffer_.reset(new (&checker) char[op_size_]);
  if (!checker.check()) {
    return;
  }

  memset(raw_buffer_.get(), 0, op_size_);
  OpHeader* header = reinterpret_cast<OpHeader*>(raw_buffer_.get());
  header->operation = this;
  header->test = test_;
}

// Provides control primitives for tests that issue IO requests to the device.
class NandDeviceTest : public RamNandTest {
 public:
  NandDeviceTest() = default;
  ~NandDeviceTest() {}

 protected:
  static void CompletionCb(void* cookie, zx_status_t status, nand_operation_t* op) {
    OpHeader* header = reinterpret_cast<OpHeader*>(reinterpret_cast<char*>(op) - sizeof(OpHeader));

    header->operation->OnCompletion(status);
    header->test->operations_completed_count_++;
    sync_completion_signal(&header->test->operation_completed_);
  }

  void CreateAndQueryDevice(fit::callback<void(ddk::NandProtocolClient, size_t)> task) {
    fuchsia_hardware_nand::wire::RamNandInfo config = {};
    config.nand_info = {.page_size = kPageSize,
                        .pages_per_block = kBlockSize,
                        .num_blocks = kNumBlocks,
                        .ecc_bits = 6,
                        .oob_size = kOobSize};  // 6 bits of ECC.

    const std::string device_name = CreateDevice(std::move(config));
    WithNand(device_name, [task = std::move(task)](ddk::NandProtocolClient nand) mutable {
      size_t operation_size;
      nand_info_t info;
      nand.Query(&info, &operation_size);
      task(nand, operation_size);
    });
  }

  bool WaitForOperationToComplete() {
    zx_status_t status = sync_completion_wait(&operation_completed_, ZX_SEC(5));
    sync_completion_reset(&operation_completed_);
    return status == ZX_OK;
  }

  bool WaitForOperationsToComplete(int operations_completed_count) {
    while (operations_completed_count_ < operations_completed_count) {
      if (!WaitForOperationToComplete()) {
        return false;
      }
    }
    return true;
  }

 private:
  sync_completion_t operation_completed_;
  std::atomic<int> operations_completed_count_ = 0;
  DISALLOW_COPY_ASSIGN_AND_MOVE(NandDeviceTest);
};

// Tests trivial attempts to queue one operation.
TEST_F(NandDeviceTest, QueueOne) {
  CreateAndQueryDevice([&](ddk::NandProtocolClient device, size_t op_size) -> void {
    Operation operation(op_size, this);
    nand_operation_t* op = operation.GetOperation();
    ASSERT_TRUE(op);

    op->rw.command = NAND_OP_WRITE;
    device.Queue(op, &NandDeviceTest::CompletionCb, nullptr);

    ASSERT_TRUE(WaitForOperationToComplete());
    ASSERT_EQ(ZX_ERR_OUT_OF_RANGE, operation.status());

    op->rw.length = 1;
    device.Queue(op, &NandDeviceTest::CompletionCb, nullptr);
    ASSERT_TRUE(WaitForOperationToComplete());
    ASSERT_EQ(ZX_ERR_BAD_HANDLE, operation.status());

    op->rw.offset_nand = kNumPages;
    device.Queue(op, &NandDeviceTest::CompletionCb, nullptr);
    ASSERT_TRUE(WaitForOperationToComplete());
    ASSERT_EQ(ZX_ERR_OUT_OF_RANGE, operation.status());

    ASSERT_TRUE(operation.SetDataVmo());

    op->rw.offset_nand = kNumPages - 1;
    device.Queue(op, &NandDeviceTest::CompletionCb, nullptr);
    ASSERT_TRUE(WaitForOperationToComplete());
    ASSERT_OK(operation.status());
  });
}

// Verifies that the buffer pointed to by the operation's vmo contains the given
// pattern for the desired number of pages, skipping the pages before start.
void CheckPattern(uint8_t what, int start, int num_pages, const Operation& operation) {
  const size_t byte_count = kPageSize * num_pages;
  const size_t start_addr = kPageSize * start;
  const std::span buffer(operation.buffer() + start_addr, byte_count);
  for (const char byte : buffer) {
    ASSERT_EQ(static_cast<uint8_t>(byte), what);
  }
}

// Prepares the operation to write num_pages starting at offset.
void SetForWrite(int offset, int num_pages, Operation* operation) {
  nand_operation_t* op = operation->GetOperation();
  op->rw.command = NAND_OP_WRITE;
  op->rw.length = num_pages;
  op->rw.offset_nand = offset;
}

// Prepares the operation to read num_pages starting at offset.
void SetForRead(int offset, int num_pages, Operation* operation) {
  nand_operation_t* op = operation->GetOperation();
  op->rw.command = NAND_OP_READ;
  op->rw.length = num_pages;
  op->rw.offset_nand = offset;
}

TEST_F(NandDeviceTest, ReadWrite) {
  CreateAndQueryDevice([&](ddk::NandProtocolClient device, size_t op_size) -> void {
    Operation operation(op_size, this);
    ASSERT_TRUE(operation.SetDataVmo());
    memset(operation.buffer(), 0x55, operation.buffer_size());

    nand_operation_t* op = operation.GetOperation();
    op->rw.corrected_bit_flips = 125;

    SetForWrite(4, 4, &operation);
    device.Queue(op, &NandDeviceTest::CompletionCb, nullptr);

    ASSERT_TRUE(WaitForOperationToComplete());
    ASSERT_OK(operation.status());
    ASSERT_EQ(125u, op->rw.corrected_bit_flips);  // Doesn't modify the value.

    op->rw.command = NAND_OP_READ;
    memset(operation.buffer(), 0, operation.buffer_size());

    device.Queue(op, &NandDeviceTest::CompletionCb, nullptr);
    ASSERT_TRUE(WaitForOperationToComplete());
    ASSERT_OK(operation.status());
    ASSERT_EQ(0u, op->rw.corrected_bit_flips);
    CheckPattern(0x55, 0, 4, operation);
  });
}

// Tests that a new device is filled with 0xff (as a new nand chip).
TEST_F(NandDeviceTest, NewChip) {
  CreateAndQueryDevice([&](ddk::NandProtocolClient device, size_t op_size) -> void {
    Operation operation(op_size, this);
    ASSERT_TRUE(operation.SetDataVmo());
    ASSERT_TRUE(operation.SetOobVmo());
    memset(operation.buffer(), 0x55, operation.buffer_size());

    nand_operation_t* op = operation.GetOperation();
    op->rw.corrected_bit_flips = 125;

    SetForRead(0, kNumPages, &operation);
    op->rw.offset_oob_vmo = kNumPages;
    device.Queue(op, &NandDeviceTest::CompletionCb, nullptr);

    ASSERT_TRUE(WaitForOperationToComplete());
    ASSERT_OK(operation.status());
    ASSERT_EQ(0u, op->rw.corrected_bit_flips);

    CheckPattern(0xff, 0, kNumPages, operation);

    // Verify OOB area.
    memset(operation.buffer(), 0xff, kOobSize * kNumPages);
    const std::span buffer1(operation.buffer() + (kPageSize * kNumPages), kOobSize * kNumPages);
    const std::span buffer2(operation.buffer(), kOobSize * kNumPages);
    EXPECT_THAT(buffer1, ::testing::ElementsAreArray(buffer2));
  });
}

// Tests serialization of multiple reads and writes.
TEST_F(NandDeviceTest, QueueMultiple) {
  CreateAndQueryDevice([&](ddk::NandProtocolClient device, size_t op_size) -> void {
    std::unique_ptr<Operation> operations[10];
    for (int i = 0; i < 10; i++) {
      fbl::AllocChecker checker;
      operations[i].reset(new (&checker) Operation(op_size, this));
      ASSERT_TRUE(checker.check());
      Operation& operation = *operations[i];
      ASSERT_TRUE(operation.SetDataVmo());
      memset(operation.buffer(), i + 30, operation.buffer_size());
    }

    SetForWrite(0, 1, operations[0].get());  // 0 x x x x x
    SetForWrite(1, 3, operations[1].get());  // 0 1 1 1 x x
    SetForRead(0, 4, operations[2].get());
    SetForWrite(4, 2, operations[3].get());  // 0 1 1 1 3 3
    SetForRead(2, 4, operations[4].get());
    SetForWrite(2, 2, operations[5].get());  // 0 1 5 5 3 3
    SetForRead(0, 4, operations[6].get());
    SetForWrite(0, 4, operations[7].get());  // 7 7 7 7 3 3
    SetForRead(2, 4, operations[8].get());
    SetForRead(0, 2, operations[9].get());

    for (const auto& operation : operations) {
      nand_operation_t* op = operation->GetOperation();
      device.Queue(op, &NandDeviceTest::CompletionCb, nullptr);
    }

    ASSERT_TRUE(WaitForOperationsToComplete(10));

    for (const auto& operation : operations) {
      ASSERT_OK(operation->status());
      ASSERT_TRUE(operation->completed());
    }

    CheckPattern(30, 0, 1, *operations[2]);
    CheckPattern(31, 1, 3, *operations[2]);

    CheckPattern(31, 0, 2, *operations[4]);
    CheckPattern(33, 2, 2, *operations[4]);

    CheckPattern(30, 0, 1, *operations[6]);
    CheckPattern(31, 1, 1, *operations[6]);
    CheckPattern(35, 2, 2, *operations[6]);

    CheckPattern(37, 0, 2, *operations[8]);
    CheckPattern(33, 2, 2, *operations[8]);

    CheckPattern(37, 0, 2, *operations[9]);
  });
}

TEST_F(NandDeviceTest, OobLimits) {
  CreateAndQueryDevice([&](ddk::NandProtocolClient device, size_t op_size) -> void {
    Operation operation(op_size, this);
    nand_operation_t* op = operation.GetOperation();
    op->rw.command = NAND_OP_READ;
    op->rw.offset_oob_vmo = 0;

    device.Queue(op, &NandDeviceTest::CompletionCb, nullptr);
    ASSERT_TRUE(WaitForOperationToComplete());
    ASSERT_EQ(ZX_ERR_OUT_OF_RANGE, operation.status());

    op->rw.length = 1;
    device.Queue(op, &NandDeviceTest::CompletionCb, nullptr);
    ASSERT_TRUE(WaitForOperationToComplete());
    ASSERT_EQ(ZX_ERR_BAD_HANDLE, operation.status());

    op->rw.offset_nand = kNumPages;
    device.Queue(op, &NandDeviceTest::CompletionCb, nullptr);
    ASSERT_TRUE(WaitForOperationToComplete());
    ASSERT_EQ(ZX_ERR_OUT_OF_RANGE, operation.status());

    ASSERT_TRUE(operation.SetOobVmo());

    op->rw.offset_nand = kNumPages - 1;
    device.Queue(op, &NandDeviceTest::CompletionCb, nullptr);
    ASSERT_TRUE(WaitForOperationToComplete());
    ASSERT_OK(operation.status());

    op->rw.length = 5;
    device.Queue(op, &NandDeviceTest::CompletionCb, nullptr);
    ASSERT_TRUE(WaitForOperationToComplete());
    ASSERT_EQ(ZX_ERR_OUT_OF_RANGE, operation.status());
  });
}

TEST_F(NandDeviceTest, ReadWriteOob) {
  CreateAndQueryDevice([&](ddk::NandProtocolClient device, size_t op_size) -> void {
    Operation operation(op_size, this);
    ASSERT_TRUE(operation.SetOobVmo());

    static const std::array<uint8_t, kOobSize> kDesired = {'a', 'b', 'c', 'd'};
    memcpy(operation.buffer(), kDesired.data(), kOobSize);

    nand_operation_t* op = operation.GetOperation();
    op->rw.corrected_bit_flips = 125;

    SetForWrite(2, 1, &operation);
    op->rw.offset_oob_vmo = 0;
    device.Queue(op, &NandDeviceTest::CompletionCb, nullptr);

    ASSERT_TRUE(WaitForOperationToComplete());
    ASSERT_OK(operation.status());
    ASSERT_EQ(125u, op->rw.corrected_bit_flips);  // Doesn't modify the value.

    op->rw.command = NAND_OP_READ;
    op->rw.length = 2;
    op->rw.offset_nand = 1;
    memset(operation.buffer(), 0, kOobSize * 2);

    device.Queue(op, &NandDeviceTest::CompletionCb, nullptr);
    ASSERT_TRUE(WaitForOperationToComplete());
    ASSERT_OK(operation.status());
    ASSERT_EQ(0u, op->rw.corrected_bit_flips);

    // The "second page" has the data of interest.
    const std::span buffer(operation.buffer() + kOobSize, kOobSize);
    EXPECT_THAT(buffer, ::testing::ElementsAreArray(kDesired));
  });
}

TEST_F(NandDeviceTest, ReadWriteDataAndOob) {
  CreateAndQueryDevice([&](ddk::NandProtocolClient device, size_t op_size) -> void {
    Operation operation(op_size, this);
    ASSERT_TRUE(operation.SetDataVmo());
    ASSERT_TRUE(operation.SetOobVmo());

    memset(operation.buffer(), 0x55, kPageSize * 2);
    memset(operation.buffer() + (kPageSize * 2), 0xaa, kOobSize * 2);

    nand_operation_t* op = operation.GetOperation();
    op->rw.corrected_bit_flips = 125;

    SetForWrite(2, 2, &operation);
    op->rw.offset_oob_vmo = 2;  // OOB is right after data.
    device.Queue(op, &NandDeviceTest::CompletionCb, nullptr);

    ASSERT_TRUE(WaitForOperationToComplete());
    ASSERT_OK(operation.status());
    ASSERT_EQ(125u, op->rw.corrected_bit_flips);  // Doesn't modify the value.

    op->rw.command = NAND_OP_READ;
    memset(operation.buffer(), 0, kPageSize * 4);

    device.Queue(op, &NandDeviceTest::CompletionCb, nullptr);
    ASSERT_TRUE(WaitForOperationToComplete());
    ASSERT_OK(operation.status());
    ASSERT_EQ(0u, op->rw.corrected_bit_flips);

    // Verify data.
    CheckPattern(0x55, 0, 2, operation);

    // Verify OOB.
    memset(operation.buffer(), 0xaa, kPageSize);
    const std::span buffer1(operation.buffer() + (kPageSize * 2), kOobSize * 2);
    const std::span buffer2(operation.buffer(), kOobSize * 2);
    EXPECT_THAT(buffer1, ::testing::ElementsAreArray(buffer2));
  });
}

TEST_F(NandDeviceTest, ReadWriteDataBytes) {
  CreateAndQueryDevice([&](ddk::NandProtocolClient device, size_t op_size) -> void {
    Operation operation(op_size, this);
    nand_operation_t* op = operation.GetOperation();
    op->rw_bytes.command = NAND_OP_WRITE_BYTES;
    op->rw_bytes.length = 2 * kPageSize;
    op->rw_bytes.offset_nand = 2 * kPageSize;
    ASSERT_TRUE(operation.SetDataVmo());

    memset(operation.buffer(), 0x55, kPageSize * 2);

    device.Queue(op, &NandDeviceTest::CompletionCb, nullptr);

    ASSERT_TRUE(WaitForOperationToComplete());
    ASSERT_OK(operation.status());

    op->rw_bytes.command = NAND_OP_READ_BYTES;
    memset(operation.buffer(), 0, kPageSize * 4);

    device.Queue(op, &NandDeviceTest::CompletionCb, nullptr);
    ASSERT_TRUE(WaitForOperationToComplete());
    ASSERT_OK(operation.status());

    // Verify data.
    CheckPattern(0x55, 0, 2, operation);
  });
}

TEST_F(NandDeviceTest, EraseLimits) {
  CreateAndQueryDevice([&](ddk::NandProtocolClient device, size_t op_size) -> void {
    Operation operation(op_size, this);
    ASSERT_TRUE(operation.SetDataVmo());

    nand_operation_t* op = operation.GetOperation();
    op->erase.command = NAND_OP_ERASE;

    device.Queue(op, &NandDeviceTest::CompletionCb, nullptr);
    ASSERT_TRUE(WaitForOperationToComplete());
    ASSERT_EQ(ZX_ERR_OUT_OF_RANGE, operation.status());

    op->erase.first_block = 5;
    op->erase.num_blocks = 1;
    device.Queue(op, &NandDeviceTest::CompletionCb, nullptr);
    ASSERT_TRUE(WaitForOperationToComplete());
    ASSERT_EQ(ZX_ERR_OUT_OF_RANGE, operation.status());

    op->erase.first_block = 4;
    op->erase.num_blocks = 2;
    device.Queue(op, &NandDeviceTest::CompletionCb, nullptr);
    ASSERT_TRUE(WaitForOperationToComplete());
    ASSERT_EQ(ZX_ERR_OUT_OF_RANGE, operation.status());
  });
}

TEST_F(NandDeviceTest, Erase) {
  CreateAndQueryDevice([&](ddk::NandProtocolClient device, size_t op_size) -> void {
    Operation operation(op_size, this);
    nand_operation_t* op = operation.GetOperation();
    op->erase.command = NAND_OP_ERASE;
    op->erase.first_block = 3;
    op->erase.num_blocks = 2;

    device.Queue(op, &NandDeviceTest::CompletionCb, nullptr);
    ASSERT_TRUE(WaitForOperationToComplete());
    ASSERT_OK(operation.status());

    memset(op, 0, sizeof(*op));
    SetForRead(0, kNumPages, &operation);
    ASSERT_TRUE(operation.SetDataVmo());
    ASSERT_TRUE(operation.SetOobVmo());
    op->rw.offset_oob_vmo = kNumPages;
    device.Queue(op, &NandDeviceTest::CompletionCb, nullptr);

    ASSERT_TRUE(WaitForOperationToComplete());
    ASSERT_OK(operation.status());
    CheckPattern(0xff, 0, kNumPages, operation);

    // Verify OOB area.
    memset(operation.buffer(), 0xff, kOobSize * kNumPages);
    const std::span buffer1(operation.buffer() + (kPageSize * kNumPages), kOobSize * kNumPages);
    const std::span buffer2(operation.buffer(), kOobSize * kNumPages);
    EXPECT_THAT(buffer1, ::testing::ElementsAreArray(buffer2));
  });
}

TEST_F(NandDeviceTest, WearVmo) {
  fuchsia_hardware_nand::wire::RamNandInfo config = BuildConfig();
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(kNumBlocks * sizeof(uint32_t), 0, &vmo));
  ASSERT_OK(vmo.duplicate(ZX_DEFAULT_VMO_RIGHTS, &config.wear_vmo));
  const std::string device_name = CreateDevice(std::move(config));

  WithNand(device_name, [&](ddk::NandProtocolClient device) {
    // Starts as zero.
    uint32_t result;
    ASSERT_OK(vmo.read(&result, 0, sizeof(uint32_t)));
    ASSERT_EQ(result, 0u);

    size_t op_size;
    nand_info_t info;
    device.Query(&info, &op_size);
    Operation operation(op_size, this);
    nand_operation_t* op = operation.GetOperation();
    op->erase.command = NAND_OP_ERASE;
    op->erase.first_block = 0;
    op->erase.num_blocks = 1;

    device.Queue(op, &NandDeviceTest::CompletionCb, nullptr);
    ASSERT_TRUE(WaitForOperationToComplete());
    ASSERT_OK(operation.status());

    // Incremented for first block.
    ASSERT_OK(vmo.read(&result, 0, sizeof(uint32_t)));
    ASSERT_EQ(result, 1u);
  });
}

TEST_F(RamNandTest, InvalidConfigOverflow) {
  // page_size = 0
  {
    fuchsia_hardware_nand::wire::RamNandInfo config = BuildConfig();
    config.nand_info.page_size = 0;
    fidl::WireResult result = ram_nand_ctl()->CreateDevice(std::move(config));
    ASSERT_OK(result.status());
    EXPECT_EQ(result->status, ZX_ERR_INVALID_ARGS);
  }

  // pages_per_block * num_blocks overflows uint32_t
  {
    fuchsia_hardware_nand::wire::RamNandInfo config = BuildConfig();
    config.nand_info.pages_per_block = 0x10000;
    config.nand_info.num_blocks =
        0x100000;  // 0x10000 * 0x100000 = 0x1'000'000'000 (overflows uint32_t)
    fidl::WireResult result = ram_nand_ctl()->CreateDevice(std::move(config));
    ASSERT_OK(result.status());
    EXPECT_EQ(result->status, ZX_ERR_INVALID_ARGS);
  }

  // page_size + oob_size overflows uint32_t
  {
    fuchsia_hardware_nand::wire::RamNandInfo config = BuildConfig();
    config.nand_info.page_size = 0xffffffff;
    config.nand_info.oob_size = 0x10;
    fidl::WireResult result = ram_nand_ctl()->CreateDevice(std::move(config));
    ASSERT_OK(result.status());
    EXPECT_EQ(result->status, ZX_ERR_INVALID_ARGS);
  }
}

}  // namespace ram_nand::testing
