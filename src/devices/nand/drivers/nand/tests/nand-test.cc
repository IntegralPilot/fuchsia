// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/nand/drivers/nand/nand.h"

#include <lib/driver/compat/cpp/banjo_client.h>
#include <lib/driver/compat/cpp/device_server.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/fzl/owned-vmo-mapper.h>
#include <lib/sync/completion.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <zircon/errors.h>

#include <atomic>
#include <cstdint>
#include <memory>
#include <utility>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "fuchsia/hardware/nand/c/banjo.h"
#include "lib/inspect/cpp/hierarchy.h"
#include "lib/inspect/cpp/inspector.h"
#include "lib/inspect/cpp/reader.h"
#include "lib/inspect/cpp/vmo/types.h"
#include "src/lib/testing/predicates/status.h"

namespace nand::testing {

constexpr uint32_t kPageSize = 1024;
constexpr uint32_t kOobSize = 8;
constexpr uint32_t kNumPages = 20;
constexpr uint32_t kNumBlocks = 10;
constexpr uint32_t kEccBits = 10;
constexpr uint32_t kNumOobSize = 8;

constexpr uint8_t kMagic = 'd';
constexpr uint8_t kOobMagic = 'o';

constexpr nand_info_t kInfo = {
    .page_size = kPageSize,
    .pages_per_block = kNumPages,
    .num_blocks = kNumBlocks,
    .ecc_bits = kEccBits,
    .oob_size = kNumOobSize,
    .nand_class = 0,
    .partition_guid = {0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15}};

enum class OperationType {
  kRead,
  kWrite,
  kErase,
};

struct LastOperation {
  OperationType type;
  uint32_t nandpage;
};

// Fake for the raw nand protocol.
class FakeRawNand : public ddk::RawNandProtocol<FakeRawNand> {
 public:
  FakeRawNand() : proto_({&raw_nand_protocol_ops_, this}) {}

  const raw_nand_protocol_t* proto() const { return &proto_; }

  void set_result(zx_status_t result) { result_ = result; }
  void set_ecc_bits(uint32_t ecc_bits) { ecc_bits_ = ecc_bits; }
  void set_read_callback(std::function<void(FakeRawNand*)> read_callback) {
    read_callback_ = std::move(read_callback);
  }

  // Raw nand protocol:
  zx_status_t RawNandGetNandInfo(nand_info_t* out_info) {
    *out_info = info_;
    return result_;
  }

  zx_status_t RawNandReadPageHwecc(uint32_t nandpage, uint8_t* out_data_buffer, size_t data_size,
                                   size_t* out_data_actual, uint8_t* out_oob_buffer,
                                   size_t oob_size, size_t* out_oob_actual,
                                   uint32_t* out_ecc_correct) {
    if (read_callback_)
      read_callback_(this);
    if (nandpage > info_.pages_per_block * info_.num_blocks) {
      result_ = ZX_ERR_IO;
    }

    // The real implementation handles these being null, so should the fake.
    if (out_data_buffer) {
      static_cast<uint8_t*>(out_data_buffer)[0] = kMagic;
    }
    if (out_oob_buffer) {
      static_cast<uint8_t*>(out_oob_buffer)[0] = kOobMagic;
    }
    *out_ecc_correct = ecc_bits_;

    std::lock_guard<std::mutex> lock(lock_);
    last_op_.type = OperationType::kRead;
    last_op_.nandpage = nandpage;

    return result_;
  }

  zx_status_t RawNandWritePageHwecc(const uint8_t* data_buffer, size_t data_size,
                                    const uint8_t* oob_buffer, size_t oob_size, uint32_t nandpage) {
    if (nandpage > info_.pages_per_block * info_.num_blocks) {
      result_ = ZX_ERR_IO;
    }

    uint8_t byte = static_cast<const uint8_t*>(data_buffer)[0];
    if (byte != kMagic) {
      result_ = ZX_ERR_IO;
    }

    byte = static_cast<const uint8_t*>(oob_buffer)[0];
    if (byte != kOobMagic) {
      result_ = ZX_ERR_IO;
    }

    std::lock_guard<std::mutex> lock(lock_);
    last_op_.type = OperationType::kWrite;
    last_op_.nandpage = nandpage;

    return result_;
  }

  zx_status_t RawNandEraseBlock(uint32_t nandpage) {
    std::lock_guard<std::mutex> lock(lock_);
    last_op_.type = OperationType::kErase;
    last_op_.nandpage = nandpage;
    return result_;
  }

  LastOperation last_op() {
    std::lock_guard<std::mutex> lock(lock_);
    return last_op_;
  }

  compat::DeviceServer::BanjoConfig GetBanjoConfig() {
    compat::DeviceServer::BanjoConfig config{.default_proto_id = ZX_PROTOCOL_RAW_NAND};
    config.callbacks[ZX_PROTOCOL_RAW_NAND] = banjo_server_.callback();
    return config;
  }

 private:
  raw_nand_protocol_t proto_;
  nand_info_t info_ = kInfo;
  zx_status_t result_ = ZX_OK;
  uint32_t ecc_bits_ = 0;
  // Calls a specified callback passing "this" at the beginning of the RawNandReadPageHwecc.
  std::function<void(FakeRawNand*)> read_callback_ = {};

  std::mutex lock_;
  LastOperation last_op_ TA_GUARDED(lock_) = {};

  compat::BanjoServer banjo_server_{ZX_PROTOCOL_RAW_NAND, this, &raw_nand_protocol_ops_};
};

class NandTestEnvironment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    async_dispatcher_t* dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();

    device_server_.Initialize("default", std::nullopt, raw_nand_.GetBanjoConfig());
    if (zx_status_t status = device_server_.Serve(dispatcher, &to_driver_vfs); status != ZX_OK) {
      return zx::error(status);
    }

    return zx::ok();
  }

  FakeRawNand& raw_nand() { return raw_nand_; }

 private:
  FakeRawNand raw_nand_;
  compat::DeviceServer device_server_;
};

// Wrapper around `NandDriver` needed in order to expose the driver's inspect data.
class TestNandDriver : public NandDriver {
 public:
  explicit TestNandDriver() : NandDriver() {}

  static DriverRegistration GetDriverRegistration() {
    return FUCHSIA_DRIVER_REGISTRATION_V1(fdf_internal::DriverServer2<TestNandDriver>::initialize,
                                          fdf_internal::DriverServer2<TestNandDriver>::destroy);
  }

  inspect::ComponentInspector& inspector() { return component_inspector().value(); }
};

class FixtureConfig final {
 public:
  using DriverType = TestNandDriver;
  using EnvironmentType = NandTestEnvironment;
};

class NandDriverTest;

// Wrapper for a nand_operation_t.
class Operation {
 public:
  explicit Operation(size_t op_size, NandDriverTest* test) : op_size_(op_size), test_(test) {}
  ~Operation() {}

  // Accessors for the memory represented by the operation's vmo.
  size_t buffer_size() const { return buffer_size_; }
  void* buffer() const { return data_mapper_.start(); }

  size_t oob_buffer_size() const { return buffer_size_; }
  void* oob_buffer() const { return oob_mapper_.start(); }

  // Creates a vmo and sets the handle on the nand_operation_t.
  bool SetVmo();
  bool SetDataVmo();
  bool SetOobVmo();

  nand_operation_t* GetOperation();

  void OnCompletion(zx_status_t status) {
    status_ = status;
    completed_ = true;
  }

  bool completed() const { return completed_; }
  zx_status_t status() const { return status_; }
  NandDriverTest* test() const { return test_; }

  DISALLOW_COPY_ASSIGN_AND_MOVE(Operation);

 private:
  zx_handle_t GetDataVmo();
  zx_handle_t GetOobVmo();

  fzl::OwnedVmoMapper data_mapper_;
  fzl::OwnedVmoMapper oob_mapper_;
  size_t op_size_;
  NandDriverTest* test_;
  zx_status_t status_ = ZX_ERR_ACCESS_DENIED;
  bool completed_ = false;
  static constexpr size_t buffer_size_ = kNumBlocks * kPageSize * kNumPages;
  static constexpr size_t oob_buffer_size_ = kNumBlocks * kPageSize * kNumPages;
  std::unique_ptr<char[]> raw_buffer_;
};

bool Operation::SetVmo() { return SetDataVmo() && SetOobVmo(); }

bool Operation::SetDataVmo() {
  nand_operation_t* operation = GetOperation();
  if (!operation) {
    return false;
  }
  operation->rw.data_vmo = GetDataVmo();
  return operation->rw.data_vmo != ZX_HANDLE_INVALID;
}

bool Operation::SetOobVmo() {
  nand_operation_t* operation = GetOperation();
  if (!operation) {
    return false;
  }
  operation->rw.oob_vmo = GetOobVmo();
  return operation->rw.oob_vmo != ZX_HANDLE_INVALID;
}

nand_operation_t* Operation::GetOperation() {
  if (!raw_buffer_) {
    raw_buffer_.reset(new char[op_size_]);
    memset(raw_buffer_.get(), 0, op_size_);
  }
  return reinterpret_cast<nand_operation_t*>(raw_buffer_.get());
}

zx_handle_t Operation::GetDataVmo() {
  if (data_mapper_.start()) {
    return data_mapper_.vmo().get();
  }

  if (data_mapper_.CreateAndMap(buffer_size_, "") != ZX_OK) {
    return ZX_HANDLE_INVALID;
  }

  return data_mapper_.vmo().get();
}

zx_handle_t Operation::GetOobVmo() {
  if (oob_mapper_.start()) {
    return oob_mapper_.vmo().get();
  }

  if (oob_mapper_.CreateAndMap(oob_buffer_size_, "") != ZX_OK) {
    return ZX_HANDLE_INVALID;
  }

  return oob_mapper_.vmo().get();
}

class NandDriverTest : public ::testing::Test {
 public:
  static void CompletionCb(void* cookie, zx_status_t status, nand_operation_t* op) {
    Operation* operation = reinterpret_cast<Operation*>(cookie);

    operation->OnCompletion(status);
    operation->test()->num_completed_++;
    sync_completion_signal(&operation->test()->event_);
  }

  void SetUp() override {
    static const uint64_t kProcessKoid = compat::internal::GetKoid();

    ASSERT_OK(driver_test_.StartDriver());

    zx::result compat_client_end =
        driver_test_.Connect<fuchsia_driver_compat::Service::Device>(NandDriver::kChildNodeName);
    EXPECT_OK(compat_client_end);
    fidl::WireClient<fuchsia_driver_compat::Device> compat(
        std::move(compat_client_end.value()),
        driver_test_.runtime().GetForegroundDispatcher()->async_dispatcher());

    zx::result<ddk::NandProtocolClient> nand;
    compat->GetBanjoProtocol(ddk::NandProtocolClient::kProtocolId, kProcessKoid)
        .ThenExactlyOnce(
            [&](fidl::WireUnownedResult<fuchsia_driver_compat::Device::GetBanjoProtocol>& result) {
              nand = compat::internal::OnResult<ddk::NandProtocolClient>(result);
            });
    driver_test_.runtime().RunUntilIdle();
    ASSERT_OK(nand);
    ASSERT_TRUE(nand.value().is_valid());
    nand_ = nand.value();

    nand_info_t info;
    nand_.Query(&info, &op_size_);
  }

  void TearDown() override { ASSERT_OK(driver_test_.StopDriver()); }

 protected:
  void Wait() {
    ASSERT_EQ(sync_completion_wait(&event_, ZX_TIME_INFINITE), ZX_OK);
    sync_completion_reset(&event_);
  }

  void WaitFor(int desired) {
    while (num_completed_ < desired) {
      Wait();
    }
  }

  ddk::NandProtocolClient& nand() { return nand_; }
  size_t op_size() const { return op_size_; }
  fdf_testing::ForegroundDriverTest<FixtureConfig>& driver_test() { return driver_test_; }

 private:
  sync_completion_t event_;
  std::atomic<int> num_completed_ = 0;
  fdf_testing::ForegroundDriverTest<FixtureConfig> driver_test_;
  ddk::NandProtocolClient nand_;
  size_t op_size_;
};

// Verify that the nand driver can start and stop without error.
TEST_F(NandDriverTest, StartStop) {}

TEST_F(NandDriverTest, Query) {
  nand_info_t info;
  size_t operation_size;
  nand().Query(&info, &operation_size);

  ASSERT_GT(operation_size, sizeof(nand_operation_t));
  ASSERT_EQ(info.pages_per_block, kInfo.pages_per_block);
  ASSERT_EQ(info.num_blocks, kInfo.num_blocks);
  ASSERT_EQ(info.ecc_bits, kInfo.ecc_bits);
  ASSERT_EQ(info.oob_size, kInfo.oob_size);
  ASSERT_EQ(info.nand_class, kInfo.nand_class);
  ASSERT_THAT(info.partition_guid, ::testing::ElementsAreArray(kInfo.partition_guid));
}

// Tests trivial attempts to queue one operation.
TEST_F(NandDriverTest, QueueOne) {
  Operation operation(op_size(), this);

  nand_operation_t* op = operation.GetOperation();
  ASSERT_NE(op, nullptr);

  op->rw.command = NAND_OP_READ;
  nand().Queue(op, &NandDriverTest::CompletionCb, &operation);

  Wait();
  ASSERT_EQ(ZX_ERR_OUT_OF_RANGE, operation.status());

  op->rw.length = 1;
  nand().Queue(op, &NandDriverTest::CompletionCb, &operation);
  Wait();
  ASSERT_EQ(ZX_ERR_BAD_HANDLE, operation.status());

  op->rw.offset_nand = kNumPages * kNumBlocks;
  nand().Queue(op, &NandDriverTest::CompletionCb, &operation);
  Wait();
  ASSERT_STATUS(ZX_ERR_OUT_OF_RANGE, operation.status());

  ASSERT_TRUE(operation.SetVmo());

  op->rw.offset_nand = (kNumPages * kNumBlocks) - 1;
  nand().Queue(op, &NandDriverTest::CompletionCb, &operation);
  Wait();
  ASSERT_OK(operation.status());
}

TEST_F(NandDriverTest, ReadWrite) {
  Operation operation(op_size(), this);
  ASSERT_TRUE(operation.SetVmo());

  nand_operation_t* op = operation.GetOperation();
  ASSERT_NE(op, nullptr);

  op->rw.command = NAND_OP_READ;
  op->rw.length = 2;
  op->rw.offset_nand = 3;
  ASSERT_TRUE(operation.SetVmo());
  nand().Queue(op, &NandDriverTest::CompletionCb, &operation);

  Wait();
  ASSERT_OK(operation.status());

  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    EXPECT_EQ(env.raw_nand().last_op().type, OperationType::kRead);
    EXPECT_EQ(env.raw_nand().last_op().nandpage, 4u);
  });

  op->rw.command = NAND_OP_WRITE;
  op->rw.length = 4;
  op->rw.offset_nand = 5;
  memset(operation.buffer(), kMagic, kPageSize * 5);
  memset(operation.oob_buffer(), kOobMagic, kOobSize * 5);
  nand().Queue(op, &NandDriverTest::CompletionCb, &operation);

  Wait();
  ASSERT_OK(operation.status());

  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    EXPECT_EQ(env.raw_nand().last_op().type, OperationType::kWrite);
    EXPECT_EQ(env.raw_nand().last_op().nandpage, 8u);
  });
}

TEST_F(NandDriverTest, ReadWriteVmoOffsets) {
  Operation operation(op_size(), this);
  ASSERT_TRUE(operation.SetVmo());

  nand_operation_t* op = operation.GetOperation();
  ASSERT_NE(op, nullptr);

  // Test an assortment of offset and length combinations.
  for (uint32_t offset = 0; offset < kNumPages * kNumBlocks; offset += 7) {
    for (uint32_t length = 1; offset + length < kNumPages * kNumBlocks; length += 3) {
      op->rw.command = NAND_OP_READ;
      op->rw.length = length;
      op->rw.offset_nand = offset;
      op->rw.offset_data_vmo = offset;
      op->rw.offset_oob_vmo = offset;
      nand().Queue(op, &NandDriverTest::CompletionCb, &operation);

      Wait();
      ASSERT_OK(operation.status());

      driver_test().RunInEnvironmentTypeContext([&](auto& env) {
        EXPECT_EQ(env.raw_nand().last_op().type, OperationType::kRead);
        EXPECT_EQ(env.raw_nand().last_op().nandpage, offset + length - 1);
      });

      op->rw.command = NAND_OP_WRITE;
      op->rw.length = length;
      op->rw.offset_nand = offset;
      op->rw.offset_data_vmo = offset;
      op->rw.offset_oob_vmo = offset;
      memset(static_cast<uint8_t*>(operation.buffer()) + (offset * kPageSize), kMagic,
             kPageSize * length);
      memset(static_cast<uint8_t*>(operation.oob_buffer()) + (offset * kPageSize), kOobMagic,
             kOobSize * length);
      nand().Queue(op, &NandDriverTest::CompletionCb, &operation);

      Wait();
      ASSERT_OK(operation.status());

      driver_test().RunInEnvironmentTypeContext([&](auto& env) {
        EXPECT_EQ(env.raw_nand().last_op().type, OperationType::kWrite);
        EXPECT_EQ(env.raw_nand().last_op().nandpage, length + offset - 1);
      });
    }
  }
}

TEST_F(NandDriverTest, Erase) {
  Operation operation(op_size(), this);
  nand_operation_t* op = operation.GetOperation();
  ASSERT_NE(op, nullptr);

  op->erase.command = NAND_OP_ERASE;
  op->erase.num_blocks = 1;
  op->erase.first_block = 5;
  nand().Queue(op, &NandDriverTest::CompletionCb, &operation);

  Wait();
  ASSERT_OK(operation.status());

  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    EXPECT_EQ(env.raw_nand().last_op().type, OperationType::kErase);
    EXPECT_EQ(env.raw_nand().last_op().nandpage, 5 * kNumPages);
  });
}

// Tests serialization of multiple operations.
TEST_F(NandDriverTest, QueryMultiple) {
  std::unique_ptr<Operation> operations[10];
  for (int i = 0; i < 10; i++) {
    operations[i].reset(new Operation(op_size(), this));
    Operation& operation = *(operations[i].get());
    nand_operation_t* op = operation.GetOperation();
    ASSERT_NE(op, nullptr);

    op->rw.command = NAND_OP_READ;
    op->rw.length = 1;
    op->rw.offset_nand = i;
    ASSERT_TRUE(operation.SetVmo());
    nand().Queue(op, &NandDriverTest::CompletionCb, &operation);
  }

  WaitFor(10);

  for (const auto& operation : operations) {
    ASSERT_OK(operation->status());
    ASSERT_TRUE(operation->completed());
  }
}

struct ReadMetrics {
  uint64_t ecc_bit_flips[32];
  uint64_t ecc_bit_flips_overflow;
  uint64_t attempts[9];
  uint64_t attempts_overflow;
  uint64_t internal_failure;
  uint64_t failure;
};

void ExpectUintPropertyMatches(const inspect::Hierarchy* hierarchy,
                               const std::string& property_name, uint64_t value) {
  auto* property = hierarchy->node().get_property<inspect::UintPropertyValue>(property_name);
  EXPECT_NE(property, nullptr);
  EXPECT_EQ(property->value(), value);
}

void ExpectUintHistogramMatches(const inspect::Hierarchy* hierarchy,
                                const std::string& property_name, uint64_t histogram_size,
                                const uint64_t* values, uint64_t overflow) {
  auto* property = hierarchy->node().get_property<inspect::UintArrayValue>(property_name);
  ASSERT_NE(property, nullptr);
  auto histogram = property->GetBuckets();
  // Verify the overflow count.
  EXPECT_EQ(overflow, histogram.back().count);
  // Remove the underflow and overflow buckets to simplify indexing.
  histogram.pop_back();
  histogram.erase(histogram.begin());
  ASSERT_EQ(histogram.size(), histogram_size);
  for (uint64_t i = 0; i < histogram_size; i++) {
    EXPECT_EQ(values[i], histogram[i].count);
  }
}

void ExpectMetricsMatch(const ReadMetrics& expected, const zx::vmo& inspect_vmo) {
  auto base_hierarchy = inspect::ReadFromVmo(inspect_vmo).take_value();
  auto* hierarchy = base_hierarchy.GetByPath({"nand"});
  ASSERT_NE(hierarchy, nullptr);
  ExpectUintPropertyMatches(hierarchy, "read_internal_failure", expected.internal_failure);
  ExpectUintPropertyMatches(hierarchy, "read_failure", expected.failure);
  ExpectUintHistogramMatches(hierarchy, "read_ecc_bit_flips", 32, expected.ecc_bit_flips,
                             expected.ecc_bit_flips_overflow);
  ExpectUintHistogramMatches(hierarchy, "read_attempts", 9, expected.attempts,
                             expected.attempts_overflow);
}

TEST_F(NandDriverTest, ReadMetrics) {
  // Read different pages every time to avoid caching effects.
  Operation operation(op_size(), this);
  ASSERT_TRUE(operation.SetVmo());

  nand_operation_t* op = operation.GetOperation();
  ASSERT_NE(op, nullptr);

  // Check that everything is zeroes.
  ReadMetrics expected;
  memset(&expected, 0, sizeof(expected));
  zx::vmo inspect_vmo = driver_test().driver()->inspector().inspector().DuplicateVmo();
  ExpectMetricsMatch(expected, inspect_vmo);

  // Normal read.
  op->rw.command = NAND_OP_READ;
  op->rw.length = 1;
  op->rw.offset_nand = 3;
  ASSERT_TRUE(operation.SetVmo());
  nand().Queue(op, &NandDriverTest::CompletionCb, &operation);
  Wait();
  ASSERT_OK(operation.status());

  expected.attempts[1] += 1;
  expected.ecc_bit_flips[0] += 1;
  ExpectMetricsMatch(expected, inspect_vmo);

  size_t ecc_limit;
  int retries = 3;
  driver_test().RunInEnvironmentTypeContext([&](auto& env) {
    nand_info_t info;
    ASSERT_OK(env.raw_nand().RawNandGetNandInfo(&info));
    ecc_limit = info.ecc_bits;
    env.raw_nand().set_read_callback([&retries](FakeRawNand* n) {
      if (--retries == 0) {
        n->set_result(ZX_OK);
      }
    });

    // Fails ECC a few times before succeeding with bit flips.
    env.raw_nand().set_ecc_bits(4);
    env.raw_nand().set_result(ZX_ERR_IO_DATA_INTEGRITY);
  });

  op->rw.command = NAND_OP_READ;
  op->rw.length = 1;
  op->rw.offset_nand = 4;
  ASSERT_TRUE(operation.SetVmo());
  nand().Queue(op, &NandDriverTest::CompletionCb, &operation);
  Wait();
  ASSERT_OK(operation.status());

  expected.attempts[2] += 1;
  expected.internal_failure += 2;
  expected.ecc_bit_flips[ecc_limit + 1] += 2;
  expected.ecc_bit_flips[4] += 1;
  ExpectMetricsMatch(expected, inspect_vmo);

  // Fails with unexpected reason before succeeding. Should not record bit flips for failures.
  driver_test().RunInEnvironmentTypeContext(
      [&](auto& env) { env.raw_nand().set_result(ZX_ERR_BAD_STATE); });
  retries = 3;
  op->rw.command = NAND_OP_READ;
  op->rw.length = 1;
  op->rw.offset_nand = 5;
  ASSERT_TRUE(operation.SetVmo());
  nand().Queue(op, &NandDriverTest::CompletionCb, &operation);
  Wait();
  ASSERT_OK(operation.status());

  expected.attempts[2] += 1;
  expected.internal_failure += 2;
  expected.ecc_bit_flips[4] += 1;
  ExpectMetricsMatch(expected, inspect_vmo);

  // Totally fails out on retries.
  driver_test().RunInEnvironmentTypeContext(
      [&](auto& env) { env.raw_nand().set_result(ZX_ERR_IO_DATA_INTEGRITY); });
  retries = 1000000;
  op->rw.command = NAND_OP_READ;
  op->rw.length = 1;
  op->rw.offset_nand = 6;
  ASSERT_TRUE(operation.SetVmo());
  nand().Queue(op, &NandDriverTest::CompletionCb, &operation);
  Wait();
  ASSERT_NE(operation.status(), ZX_OK);

  expected.attempts_overflow += 1;
  expected.failure += 1;
  expected.internal_failure += NandDriver::kNandReadRetries;
  expected.ecc_bit_flips[ecc_limit + 1] += NandDriver::kNandReadRetries;
  ExpectMetricsMatch(expected, inspect_vmo);
}

TEST_F(NandDriverTest, ReadCacheForPoorECC) {
  Operation operation(op_size(), this);

  nand_operation_t* op = operation.GetOperation();
  ASSERT_NE(op, nullptr);

  // Normal read with no ECC errors. Should not cache.
  op->rw.command = NAND_OP_READ;
  op->rw.length = 1;
  op->rw.offset_nand = 3;
  ASSERT_TRUE(operation.SetVmo());
  nand().Queue(op, &NandDriverTest::CompletionCb, &operation);
  Wait();
  ASSERT_OK(operation.status());

  // Read the same page with moderate ECC errors. Should not cache.
  driver_test().RunInEnvironmentTypeContext([&](auto& env) { env.raw_nand().set_ecc_bits(1); });
  op->rw.command = NAND_OP_READ;
  op->rw.length = 1;
  op->rw.offset_nand = 3;
  ASSERT_TRUE(operation.SetVmo());
  nand().Queue(op, &NandDriverTest::CompletionCb, &operation);
  Wait();
  ASSERT_OK(operation.status());

  nand_info_t info;
  uint32_t ecc_limit;
  driver_test().RunInEnvironmentTypeContext([&](auto& env) {
    ASSERT_OK(env.raw_nand().RawNandGetNandInfo(&info));
    ecc_limit = info.ecc_bits;
    env.raw_nand().set_ecc_bits(ecc_limit);
  });

  // Read the same page with terrible ECC errors. Should do a normal read, but also should cache
  // for the next call.
  op->rw.command = NAND_OP_READ;
  op->rw.length = 1;
  op->rw.offset_nand = 3;
  ASSERT_TRUE(operation.SetVmo());
  nand().Queue(op, &NandDriverTest::CompletionCb, &operation);
  Wait();
  ASSERT_OK(operation.status());
  // See all the bit flips.
  ASSERT_EQ(operation.GetOperation()->rw.corrected_bit_flips, ecc_limit);

  // Read the same page again. Should get the cached result.
  driver_test().RunInEnvironmentTypeContext(
      [&](auto& env) { env.raw_nand().set_ecc_bits(ecc_limit); });
  op->rw.command = NAND_OP_READ;
  op->rw.length = 1;
  op->rw.offset_nand = 3;
  ASSERT_TRUE(operation.SetVmo());
  nand().Queue(op, &NandDriverTest::CompletionCb, &operation);
  Wait();
  ASSERT_OK(operation.status());
  // Cached result reports no bit flips.
  ASSERT_EQ(operation.GetOperation()->rw.corrected_bit_flips, 0u);
}

TEST_F(NandDriverTest, ReadCacheFailedRetry) {
  Operation operation(op_size(), this);

  nand_operation_t* op = operation.GetOperation();
  ASSERT_NE(op, nullptr);

  // Normal read with no ECC errors. Should not cache.
  op->rw.command = NAND_OP_READ;
  op->rw.length = 1;
  op->rw.offset_nand = 3;
  ASSERT_TRUE(operation.SetVmo());
  nand().Queue(op, &NandDriverTest::CompletionCb, &operation);
  Wait();
  ASSERT_OK(operation.status());

  nand_info_t info;
  int retries = 2;
  driver_test().RunInEnvironmentTypeContext([&](auto& env) {
    ASSERT_OK(env.raw_nand().RawNandGetNandInfo(&info));
    env.raw_nand().set_read_callback([&retries](FakeRawNand* n) {
      if (--retries == 0) {
        n->set_result(ZX_OK);
      }
    });
    env.raw_nand().set_ecc_bits(1);
    env.raw_nand().set_result(ZX_ERR_IO_DATA_INTEGRITY);
  });
  size_t ecc_limit = info.ecc_bits;

  // Read the same page. Fails ECC the first time before succeeding with max bit flips.
  op->rw.command = NAND_OP_READ;
  op->rw.length = 1;
  op->rw.offset_nand = 3;
  ASSERT_TRUE(operation.SetVmo());
  nand().Queue(op, &NandDriverTest::CompletionCb, &operation);
  Wait();
  ASSERT_OK(operation.status());
  ASSERT_EQ(operation.GetOperation()->rw.corrected_bit_flips, ecc_limit);

  // Read again. Prime for failures and bit flips again. It should return no bitflips this time due
  // to cache.
  driver_test().RunInEnvironmentTypeContext([&](auto& env) {
    env.raw_nand().set_ecc_bits(1);
    env.raw_nand().set_result(ZX_ERR_IO_DATA_INTEGRITY);
  });
  retries = 2;
  op->rw.command = NAND_OP_READ;
  op->rw.length = 1;
  op->rw.offset_nand = 3;
  ASSERT_TRUE(operation.SetVmo());
  nand().Queue(op, &NandDriverTest::CompletionCb, &operation);
  Wait();
  ASSERT_OK(operation.status());
  ASSERT_EQ(operation.GetOperation()->rw.corrected_bit_flips, 0u);
}

TEST_F(NandDriverTest, ReadCachePurgeOnErase) {
  Operation operation(op_size(), this);

  nand_operation_t* op = operation.GetOperation();
  ASSERT_NE(op, nullptr);

  // Normal read with no ECC errors. Should not cache.
  op->rw.command = NAND_OP_READ;
  op->rw.length = 1;
  op->rw.offset_nand = 3;
  ASSERT_TRUE(operation.SetVmo());
  nand().Queue(op, &NandDriverTest::CompletionCb, &operation);
  Wait();
  ASSERT_OK(operation.status());

  nand_info_t info;
  uint32_t ecc_limit;
  driver_test().RunInEnvironmentTypeContext([&](auto& env) {
    ASSERT_OK(env.raw_nand().RawNandGetNandInfo(&info));
    ecc_limit = info.ecc_bits;
    env.raw_nand().set_ecc_bits(ecc_limit);
  });

  // Read the same page with terrible ECC errors. Should do a normal read, but also should cache
  // for the next call.
  op->rw.command = NAND_OP_READ;
  op->rw.length = 1;
  op->rw.offset_nand = 3;
  ASSERT_TRUE(operation.SetVmo());
  nand().Queue(op, &NandDriverTest::CompletionCb, &operation);
  Wait();
  ASSERT_OK(operation.status());
  // See all the bit flips.
  ASSERT_EQ(operation.GetOperation()->rw.corrected_bit_flips, ecc_limit);

  // Read the same page again. Should get the cached result.
  driver_test().RunInEnvironmentTypeContext(
      [&](auto& env) { env.raw_nand().set_ecc_bits(ecc_limit); });
  op->rw.command = NAND_OP_READ;
  op->rw.length = 1;
  op->rw.offset_nand = 3;
  ASSERT_TRUE(operation.SetVmo());
  nand().Queue(op, &NandDriverTest::CompletionCb, &operation);
  Wait();
  ASSERT_OK(operation.status());
  // Cached result reports no bit flips.
  ASSERT_EQ(operation.GetOperation()->rw.corrected_bit_flips, 0u);

  // Purge from cache.
  Operation erase_operation(op_size(), this);
  nand_operation_t* erase_op = erase_operation.GetOperation();
  erase_op->command = NAND_OP_ERASE;
  erase_op->erase.command = NAND_OP_ERASE;
  erase_op->erase.first_block = 3 / info.pages_per_block;
  erase_op->erase.num_blocks = 1;
  nand().Queue(erase_op, &NandDriverTest::CompletionCb, &erase_operation);
  Wait();
  ASSERT_OK(erase_operation.status());

  // Get a full read without cached result, so we see bit flips.
  driver_test().RunInEnvironmentTypeContext(
      [&](auto& env) { env.raw_nand().set_ecc_bits(ecc_limit); });
  op->rw.command = NAND_OP_READ;
  op->rw.length = 1;
  op->rw.offset_nand = 3;
  ASSERT_TRUE(operation.SetVmo());
  nand().Queue(op, &NandDriverTest::CompletionCb, &operation);
  Wait();
  ASSERT_OK(operation.status());
  // See all the bit flips.
  ASSERT_EQ(operation.GetOperation()->rw.corrected_bit_flips, ecc_limit);
}

TEST_F(NandDriverTest, InsertToCacheWithNullPayloads) {
  // All uncached reads will come back with dangerous ECC.
  nand_info_t info;
  uint32_t ecc_limit;
  driver_test().RunInEnvironmentTypeContext([&](auto& env) {
    ASSERT_OK(env.raw_nand().RawNandGetNandInfo(&info));
    ecc_limit = info.ecc_bits;
    env.raw_nand().set_ecc_bits(ecc_limit);
  });

  // Test combinations of insert with null payload pointers. Read back after with both payload
  // pointers populated, and should see a cached result if the data pointer was populated for
  // insert. Reads with both payloads as null are disallowed.
  for (uint32_t i = 1; i < 4; ++i) {
    bool set_data_vmo = (i & 1) > 0;
    bool set_oob_vmo = (i & 2) > 0;

    // Initial read which may or may not cache.
    Operation insert_operation(op_size(), this);
    nand_operation_t* insert_op = insert_operation.GetOperation();
    ASSERT_NE(insert_op, nullptr);
    insert_op->rw.command = NAND_OP_READ;
    insert_op->rw.length = 1;
    insert_op->rw.offset_nand = i;
    insert_op->rw.data_vmo = ZX_HANDLE_INVALID;
    insert_op->rw.oob_vmo = ZX_HANDLE_INVALID;
    if (set_data_vmo) {
      ASSERT_TRUE(insert_operation.SetDataVmo());
    }
    if (set_oob_vmo) {
      ASSERT_TRUE(insert_operation.SetOobVmo());
    }
    nand().Queue(insert_op, &NandDriverTest::CompletionCb, &insert_operation);
    Wait();
    ASSERT_OK(insert_operation.status());
    ASSERT_EQ(insert_operation.GetOperation()->rw.corrected_bit_flips, ecc_limit);

    // Follow-on read to verify if the result is cached. Set both VMOs this time.
    Operation fetch_operation(op_size(), this);
    nand_operation_t* fetch_op = fetch_operation.GetOperation();
    ASSERT_NE(fetch_op, nullptr);
    fetch_op->rw.command = NAND_OP_READ;
    fetch_op->rw.length = 1;
    fetch_op->rw.offset_nand = i;
    ASSERT_TRUE(fetch_operation.SetVmo());
    nand().Queue(fetch_op, &NandDriverTest::CompletionCb, &fetch_operation);
    Wait();
    ASSERT_OK(fetch_operation.status());
    // We don't try to do any caching if the data wasn't fetched, so we see no bit errors due to
    // caching only when the data vmo was set.
    if (set_data_vmo) {
      ASSERT_EQ(fetch_operation.GetOperation()->rw.corrected_bit_flips, 0u);
    } else {
      ASSERT_EQ(fetch_operation.GetOperation()->rw.corrected_bit_flips, ecc_limit);
    }
    // Verify that results are correct.
    ASSERT_EQ(static_cast<uint8_t*>(fetch_operation.oob_buffer())[0], kOobMagic);
    ASSERT_EQ(static_cast<uint8_t*>(fetch_operation.buffer())[0], kMagic);
  }
}

TEST_F(NandDriverTest, FetchFromCacheWithNullPayloads) {
  // All uncached reads will come back with dangerous ECC.
  nand_info_t info;
  uint32_t ecc_limit;
  driver_test().RunInEnvironmentTypeContext([&](auto& env) {
    ASSERT_OK(env.raw_nand().RawNandGetNandInfo(&info));
    ecc_limit = info.ecc_bits;
    env.raw_nand().set_ecc_bits(ecc_limit);
  });

  // Test combinations of read with null payload pointers. Insert with both payload pointers
  // populated, and should see a cached result every time for the second lookup. Reads with both
  // payloads as null are disallowed.
  for (uint32_t i = 1; i < 4; ++i) {
    bool set_data_vmo = (i & 1) > 0;
    bool set_oob_vmo = (i & 2) > 0;

    // Initial read which sets both vmos and should always cache.
    Operation insert_operation(op_size(), this);
    nand_operation_t* insert_op = insert_operation.GetOperation();
    ASSERT_NE(insert_op, nullptr);
    insert_op->rw.command = NAND_OP_READ;
    insert_op->rw.length = 1;
    insert_op->rw.offset_nand = i;
    ASSERT_TRUE(insert_operation.SetVmo());
    nand().Queue(insert_op, &NandDriverTest::CompletionCb, &insert_operation);
    Wait();
    ASSERT_OK(insert_operation.status());
    ASSERT_EQ(insert_operation.GetOperation()->rw.corrected_bit_flips, ecc_limit);

    // Follow-on read to verify the result is fetched from cache regardless of payload
    // pointers.
    Operation fetch_operation(op_size(), this);
    nand_operation_t* fetch_op = fetch_operation.GetOperation();
    ASSERT_NE(fetch_op, nullptr);
    fetch_op->rw.command = NAND_OP_READ;
    fetch_op->rw.length = 1;
    fetch_op->rw.offset_nand = i;
    fetch_op->rw.data_vmo = ZX_HANDLE_INVALID;
    fetch_op->rw.oob_vmo = ZX_HANDLE_INVALID;
    if (set_data_vmo) {
      ASSERT_TRUE(fetch_operation.SetDataVmo());
    }
    if (set_oob_vmo) {
      ASSERT_TRUE(fetch_operation.SetOobVmo());
    }
    nand().Queue(fetch_op, &NandDriverTest::CompletionCb, &fetch_operation);
    Wait();
    ASSERT_OK(fetch_operation.status());
    if (set_data_vmo) {
      ASSERT_EQ(fetch_operation.GetOperation()->rw.corrected_bit_flips, 0u);
    } else {
      ASSERT_EQ(fetch_operation.GetOperation()->rw.corrected_bit_flips, ecc_limit);
    }

    // Verify that results are correct if we fetched them.
    if (set_oob_vmo) {
      ASSERT_EQ(static_cast<uint8_t*>(fetch_operation.oob_buffer())[0], kOobMagic);
    }
    if (set_data_vmo) {
      ASSERT_EQ(static_cast<uint8_t*>(fetch_operation.buffer())[0], kMagic);
    }
  }
}

}  // namespace nand::testing
