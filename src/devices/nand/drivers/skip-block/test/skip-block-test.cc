// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "skip-block.h"

#include <fidl/fuchsia.hardware.skipblock/cpp/wire.h>
#include <fuchsia/hardware/badblock/cpp/banjo.h>
#include <fuchsia/hardware/nand/cpp/banjo.h>
#include <lib/ddk/metadata.h>
#include <lib/driver/compat/cpp/banjo_server.h>
#include <lib/driver/compat/cpp/device_server.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/fzl/vmo-mapper.h>
#include <lib/zx/vmo.h>
#include <zircon/types.h>

#include <fbl/algorithm.h>
#include <fbl/vector.h>
#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"

namespace nand::testing {

namespace {

constexpr size_t kPageSize = 1024;
constexpr uint32_t kOobSize = 8;
constexpr uint32_t kNumPages = 20;
constexpr size_t kBlockSize = kPageSize * kNumPages;
constexpr uint32_t kNumBlocks = 10;
constexpr uint32_t kEccBits = 10;

constexpr nand_info_t kInfo = {
    .page_size = kPageSize,
    .pages_per_block = kNumPages,
    .num_blocks = kNumBlocks,
    .ecc_bits = kEccBits,
    .oob_size = kOobSize,
    .nand_class = 0,
    .partition_guid = {},
};

// Fake for the nand protocol.
class FakeNand : public ddk::NandProtocol<FakeNand> {
 public:
  void Init() {
    ASSERT_OK(mapper_.CreateAndMap(static_cast<size_t>(kNumBlocks) * kNumPages * kPageSize,
                                   ZX_VM_PERM_READ | ZX_VM_PERM_WRITE));
  }

  nand_protocol_ops_t* proto_ops() { return &nand_protocol_ops_; }

  void set_result(zx_status_t result) { result_.push_back(result); }

  // Nand protocol:
  void NandQuery(nand_info_t* info_out, size_t* nand_op_size_out) {
    *info_out = nand_info_;
    *nand_op_size_out = sizeof(nand_operation_t);
  }

  void NandQueue(nand_operation_t* op, nand_queue_callback completion_cb, void* cookie) {
    last_op_ = op->command;

    auto result = result_[call_++];

    if (result != ZX_OK) {
      completion_cb(cookie, result, op);
      return;
    }

    switch (op->command) {
      case NAND_OP_READ: {
        if (op->rw.offset_nand >= num_nand_pages_ || !op->rw.length ||
            (num_nand_pages_ - op->rw.offset_nand) < op->rw.length) {
          result = ZX_ERR_OUT_OF_RANGE;
          break;
        }
        if (op->rw.data_vmo == ZX_HANDLE_INVALID && op->rw.oob_vmo == ZX_HANDLE_INVALID) {
          result = ZX_ERR_BAD_HANDLE;
          break;
        }
        zx::unowned_vmo data_vmo(op->rw.data_vmo);
        if (*data_vmo) {
          auto* data = static_cast<uint8_t*>(mapper_.start()) + (op->rw.offset_nand * kPageSize);
          data_vmo->read(data, op->rw.offset_data_vmo, op->rw.length * kPageSize);
        }
        break;
      }
      case NAND_OP_WRITE: {
        if (op->rw.offset_nand >= num_nand_pages_ || !op->rw.length ||
            (num_nand_pages_ - op->rw.offset_nand) < op->rw.length) {
          result = ZX_ERR_OUT_OF_RANGE;
          break;
        }
        if (op->rw.data_vmo == ZX_HANDLE_INVALID && op->rw.oob_vmo == ZX_HANDLE_INVALID) {
          result = ZX_ERR_BAD_HANDLE;
          break;
        }
        zx::unowned_vmo data_vmo(op->rw.data_vmo);
        if (*data_vmo) {
          fzl::VmoMapper mapper;
          result = mapper.Map(*data_vmo, 0, 0, ZX_VM_PERM_READ);
          if (result != ZX_OK) {
            break;
          }
          auto* src = static_cast<uint8_t*>(mapper.start()) + (op->rw.offset_data_vmo * kPageSize);
          auto* dst = static_cast<uint8_t*>(mapper_.start()) + (op->rw.offset_nand * kPageSize);
          memcpy(dst, src, op->rw.length * kPageSize);
        }
        break;
      }
      case NAND_OP_ERASE: {
        if (!op->erase.num_blocks || op->erase.first_block >= nand_info_.num_blocks ||
            (op->erase.num_blocks > (nand_info_.num_blocks - op->erase.first_block))) {
          result = ZX_ERR_OUT_OF_RANGE;
          break;
        }
        auto* data = static_cast<uint8_t*>(mapper_.start()) +
                     (op->erase.first_block * kPageSize * kNumPages);
        memset(data, 0, op->erase.num_blocks * kPageSize * kNumPages);
        break;
      }
      default:
        result = ZX_ERR_NOT_SUPPORTED;
        break;
    }

    completion_cb(cookie, result, op);
  }

  zx_status_t NandGetFactoryBadBlockList(uint32_t* bad_blocks, size_t bad_block_len,
                                         size_t* num_bad_blocks) {
    *num_bad_blocks = 0;
    return ZX_OK;
  }

  void set_block_count(uint32_t num_blocks) {
    nand_info_.num_blocks = num_blocks;
    num_nand_pages_ = static_cast<size_t>(kNumPages) * num_blocks;
  }

  const fzl::VmoMapper& mapper() { return mapper_; }
  const nand_op_t& last_op() const { return last_op_; }
  compat::BanjoServer& banjo_server() { return banjo_server_; }

 private:
  size_t call_ = 0;
  nand_info_t nand_info_ = kInfo;
  fbl::Vector<zx_status_t> result_;
  size_t num_nand_pages_ = static_cast<size_t>(kNumPages) * kNumBlocks;

  fzl::VmoMapper mapper_;

  nand_op_t last_op_ = {};

  compat::BanjoServer banjo_server_{ZX_PROTOCOL_NAND, this, &nand_protocol_ops_};
};

// Fake for the bad block protocol.
class FakeBadBlock : public ddk::BadBlockProtocol<FakeBadBlock> {
 public:
  FakeBadBlock() = default;

  bad_block_protocol_ops_t* proto_ops() { return &bad_block_protocol_ops_; }

  void set_result(zx_status_t result) { result_ = result; }

  // Bad Block protocol:
  zx_status_t BadBlockGetBadBlockList(uint32_t* bad_block_list, size_t bad_block_list_len,
                                      size_t* bad_block_count) {
    *bad_block_count = grown_bad_blocks_.size();
    if (bad_block_list_len < *bad_block_count) {
      return bad_block_list == nullptr ? ZX_OK : ZX_ERR_BUFFER_TOO_SMALL;
    }
    memcpy(bad_block_list, grown_bad_blocks_.data(), grown_bad_blocks_.size() * sizeof(uint32_t));
    return result_;
  }

  zx_status_t BadBlockMarkBlockBad(uint32_t block) {
    if (result_ == ZX_OK) {
      grown_bad_blocks_.push_back(block);
    }
    return result_;
  }

  const fbl::Vector<uint32_t>& grown_bad_blocks() { return grown_bad_blocks_; }
  compat::BanjoServer& banjo_server() { return banjo_server_; }

 private:
  fbl::Vector<uint32_t> grown_bad_blocks_;
  zx_status_t result_ = ZX_OK;

  compat::BanjoServer banjo_server_{ZX_PROTOCOL_BAD_BLOCK, this, &bad_block_protocol_ops_};
};

class SkipBlockTestEnvironment : public fdf_testing::Environment {
 public:
  void Init(uint32_t count) {
    nand_.Init();

    compat::DeviceServer::BanjoConfig config{.default_proto_id = ZX_PROTOCOL_NAND};
    config.callbacks[ZX_PROTOCOL_NAND] = nand_.banjo_server().callback();
    config.callbacks[ZX_PROTOCOL_BAD_BLOCK] = bad_block_.banjo_server().callback();
    device_server_.Initialize("default", std::nullopt, std::move(config));
    device_server_.AddMetadata(DEVICE_METADATA_PRIVATE, &count, sizeof(count));
  }

  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    async_dispatcher_t* dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();
    EXPECT_OK(device_server_.Serve(dispatcher, &to_driver_vfs));

    return zx::ok();
  }

  FakeNand& nand() { return nand_; }
  FakeBadBlock& bad_block() { return bad_block_; }

 private:
  FakeNand nand_;
  FakeBadBlock bad_block_;
  compat::DeviceServer device_server_;
};

class TestConfig final {
 public:
  using DriverType = SkipBlock;
  using EnvironmentType = SkipBlockTestEnvironment;
};

class SkipBlockTest : public ::testing::Test {
 protected:
  static void CreatePayload(size_t size, zx::vmo* out) {
    zx::vmo vmo;
    fzl::VmoMapper mapper;
    ASSERT_OK(mapper.CreateAndMap(fbl::round_up(size, zx_system_get_page_size()),
                                  ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, nullptr, &vmo));
    memset(mapper.start(), 0x4a, mapper.size());
    *out = std::move(vmo);
  }

  void TearDown() override { ASSERT_OK(driver_test_.StopDriver()); }

  void StartDriver(uint32_t count = 1) {
    driver_test_.RunInEnvironmentTypeContext([&](auto& env) { env.Init(count); });
    ASSERT_OK(driver_test_.StartDriver());
    zx::result skip_block = driver_test_.Connect<fuchsia_hardware_skipblock::Service::Skipblock>();
    ASSERT_OK(skip_block);
    skip_block_.Bind(std::move(skip_block.value()),
                     driver_test_.runtime().GetForegroundDispatcher()->async_dispatcher());
  }

  void Write(ReadWriteOperation op, bool* bad_block_grown, zx_status_t expected = ZX_OK) {
    skip_block_->Write(std::move(op))
        .ThenExactlyOnce(
            [&](fidl::WireUnownedResult<fuchsia_hardware_skipblock::SkipBlock::Write>& result) {
              ASSERT_OK(result.status());
              ASSERT_STATUS(result.value().status, expected);
              *bad_block_grown = result.value().bad_block_grown;
              driver_test().runtime().Quit();
            });
    driver_test().runtime().Run();
  }

  void Read(ReadWriteOperation op, zx_status_t expected = ZX_OK) {
    skip_block_->Read(std::move(op))
        .ThenExactlyOnce(
            [&](fidl::WireUnownedResult<fuchsia_hardware_skipblock::SkipBlock::Read>& result) {
              ASSERT_OK(result.status());
              ASSERT_STATUS(result.value().status, expected);
              driver_test().runtime().Quit();
            });
    driver_test().runtime().Run();
  }

  void WriteBytes(WriteBytesOperation op, bool* bad_block_grown, zx_status_t expected = ZX_OK) {
    skip_block_->WriteBytes(std::move(op))
        .ThenExactlyOnce(
            [&](fidl::WireUnownedResult<fuchsia_hardware_skipblock::SkipBlock::WriteBytes>&
                    result) {
              ASSERT_OK(result.status());
              ASSERT_EQ(result.value().status, expected);
              *bad_block_grown = result.value().bad_block_grown;
              driver_test().runtime().Quit();
            });
    driver_test().runtime().Run();
  }

  void WriteBytesWithoutErase(WriteBytesOperation op, zx_status_t expected = ZX_OK) {
    skip_block_->WriteBytesWithoutErase(std::move(op))
        .ThenExactlyOnce(
            [&](fidl::WireUnownedResult<
                fuchsia_hardware_skipblock::SkipBlock::WriteBytesWithoutErase>& result) {
              ASSERT_OK(result.status());
              ASSERT_STATUS(result.value().status, expected);
              driver_test().runtime().Quit();
            });
    driver_test().runtime().Run();
  }

  void GetPartitionInfo(PartitionInfo* out, zx_status_t expected = ZX_OK) {
    skip_block_->GetPartitionInfo().ThenExactlyOnce(
        [&](fidl::WireUnownedResult<fuchsia_hardware_skipblock::SkipBlock::GetPartitionInfo>&
                result) {
          ASSERT_OK(result.status());
          ASSERT_STATUS(result.value().status, expected);
          *out = result.value().partition_info;
          driver_test().runtime().Quit();
        });
    driver_test().runtime().Run();
  }

  void ValidateWritten(size_t offset, size_t size) {
    driver_test_.RunInEnvironmentTypeContext([&](auto& env) {
      const uint8_t* start = static_cast<uint8_t*>(env.nand().mapper().start()) + offset;
      for (size_t i = 0; i < size; i++) {
        ASSERT_EQ(start[i], 0x4a) << "i = " << i;
      }
    });
  }

  void ValidateUnwritten(size_t offset, size_t size) {
    driver_test_.RunInEnvironmentTypeContext([&](auto& env) {
      const uint8_t* start = static_cast<uint8_t*>(env.nand().mapper().start()) + offset;
      for (size_t i = 0; i < size; i++) {
        ASSERT_EQ(start[i], 0x00) << "i = " << i;
      }
    });
  }

  fdf_testing::ForegroundDriverTest<TestConfig>& driver_test() { return driver_test_; }

 private:
  fdf_testing::ForegroundDriverTest<TestConfig> driver_test_;
  fidl::WireClient<fuchsia_hardware_skipblock::SkipBlock> skip_block_;
};

// Verify that the skip block driver can start and stop.
TEST_F(SkipBlockTest, StartStop) { StartDriver(); }

TEST_F(SkipBlockTest, GrowBadBlock) {
  StartDriver();

  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    env.nand().set_result(ZX_OK);
    env.nand().set_result(ZX_ERR_IO);
    env.nand().set_result(ZX_OK);
    env.nand().set_result(ZX_OK);
  });

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(fbl::round_up(kBlockSize, zx_system_get_page_size()), 0, &vmo));
  nand::ReadWriteOperation op = {};
  op.vmo = std::move(vmo);
  op.block = 5;
  op.block_count = 1;

  bool bad_block_grown;
  ASSERT_NO_FATAL_FAILURE(Write(std::move(op), &bad_block_grown));
  ASSERT_TRUE(bad_block_grown);
  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    ASSERT_EQ(env.bad_block().grown_bad_blocks().size(), 1u);
    ASSERT_EQ(env.bad_block().grown_bad_blocks()[0], 5u);
    ASSERT_EQ(env.nand().last_op(), NAND_OP_WRITE);
  });
}

TEST_F(SkipBlockTest, GrowMultipleBadBlock) {
  StartDriver();

  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    // Erase Block 5
    env.nand().set_result(ZX_OK);
    // Write Block 5
    env.nand().set_result(ZX_ERR_IO);
    // Erase Block 6
    env.nand().set_result(ZX_ERR_IO);
    // Erase Block 7
    env.nand().set_result(ZX_OK);
    // Write Block 7
    env.nand().set_result(ZX_OK);
  });

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(fbl::round_up(kBlockSize, zx_system_get_page_size()), 0, &vmo));
  nand::ReadWriteOperation op = {};
  op.vmo = std::move(vmo);
  op.block = 5;
  op.block_count = 1;

  bool bad_block_grown;
  ASSERT_NO_FATAL_FAILURE(Write(std::move(op), &bad_block_grown));
  ASSERT_TRUE(bad_block_grown);
  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    ASSERT_EQ(env.bad_block().grown_bad_blocks().size(), 2u);
    ASSERT_EQ(env.bad_block().grown_bad_blocks()[0], 5u);
    ASSERT_EQ(env.bad_block().grown_bad_blocks()[1], 6u);
    ASSERT_EQ(env.nand().last_op(), NAND_OP_WRITE);
  });
}

TEST_F(SkipBlockTest, MappingFailure) {
  StartDriver();

  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    // Erase Block 5
    env.nand().set_result(ZX_OK);
    // Write Block 5
    env.nand().set_result(ZX_ERR_INVALID_ARGS);
  });

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(fbl::round_up(kBlockSize, zx_system_get_page_size()), 0, &vmo));
  nand::ReadWriteOperation op = {};
  op.vmo = std::move(vmo);
  op.block = 5;
  op.block_count = 1;

  bool bad_block_grown;
  ASSERT_NO_FATAL_FAILURE(Write(std::move(op), &bad_block_grown, ZX_ERR_IO));
  ASSERT_FALSE(bad_block_grown);
  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    ASSERT_EQ(env.bad_block().grown_bad_blocks().size(), 0u);
    ASSERT_EQ(env.nand().last_op(), NAND_OP_WRITE);
  });
}

TEST_F(SkipBlockTest, WriteBytesEraseWriteMode) {
  StartDriver();

  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    // 20 pages in 1 block.
    // Write page range [110, 130], the corresponding block range is [5, 6]
    // Erase Block 5
    env.nand().set_result(ZX_OK);
    // Write Block 5
    env.nand().set_result(ZX_OK);
    // Erase Block 6
    env.nand().set_result(ZX_OK);
    // Write Block 6
    env.nand().set_result(ZX_OK);
  });

  zx::vmo vmo;
  ASSERT_NO_FATAL_FAILURE(CreatePayload(20 * kPageSize, &vmo));
  fzl::VmoMapper mapper;
  ASSERT_OK(mapper.Map(vmo, 0, 0, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE));
  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    auto start = static_cast<uint8_t*>(env.nand().mapper().start()) + (5 * kBlockSize);
    memset(start, 0xab, 2 * kBlockSize);
  });

  WriteBytesOperation op = {};
  op.vmo = std::move(vmo);
  op.offset = 110 * kPageSize;
  op.size = 20 * kPageSize;
  op.mode = WriteBytesMode::kEraseWrite;

  bool bad_block_grown;
  ASSERT_NO_FATAL_FAILURE(WriteBytes(std::move(op), &bad_block_grown));
  ASSERT_FALSE(bad_block_grown);
  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    ASSERT_EQ(env.bad_block().grown_bad_blocks().size(), 0u);
    ASSERT_EQ(env.nand().last_op(), NAND_OP_WRITE);
  });
  ASSERT_NO_FATAL_FAILURE(ValidateWritten(110 * kPageSize, 20 * kPageSize));
  ASSERT_NO_FATAL_FAILURE(ValidateUnwritten(100 * kPageSize, 10 * kPageSize));
  ASSERT_NO_FATAL_FAILURE(ValidateUnwritten(130 * kPageSize, 10 * kPageSize));
}

TEST_F(SkipBlockTest, ReadSuccess) {
  StartDriver();

  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    // Read Block 5.
    env.nand().set_result(ZX_OK);
  });

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(fbl::round_up(kBlockSize, zx_system_get_page_size()), 0, &vmo));
  nand::ReadWriteOperation op = {};
  op.vmo = std::move(vmo);
  op.block = 5;
  op.block_count = 1;

  ASSERT_NO_FATAL_FAILURE(Read(std::move(op), ZX_OK));
  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    ASSERT_EQ(env.bad_block().grown_bad_blocks().size(), 0u);
    ASSERT_EQ(env.nand().last_op(), NAND_OP_READ);
  });
}

TEST_F(SkipBlockTest, ReadFailure) {
  StartDriver();

  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    // Read Block 7.
    env.nand().set_result(ZX_ERR_INVALID_ARGS);
  });

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(fbl::round_up(kBlockSize, zx_system_get_page_size()), 0, &vmo));
  nand::ReadWriteOperation op = {};
  op.vmo = std::move(vmo);
  op.block = 7;
  op.block_count = 1;

  ASSERT_NO_FATAL_FAILURE(Read(std::move(op), ZX_ERR_IO));
  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    ASSERT_EQ(env.bad_block().grown_bad_blocks().size(), 0u);
    ASSERT_EQ(env.nand().last_op(), NAND_OP_READ);
  });
}

TEST_F(SkipBlockTest, ReadMultipleCopies) {
  driver_test().RunInEnvironmentTypeContext([](auto& env) { env.nand().set_block_count(8); });
  StartDriver(4);

  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    // Read Block 1
    env.nand().set_result(ZX_ERR_IO);
    // Read Block 3
    env.nand().set_result(ZX_ERR_IO);
    // Read Block 5
    env.nand().set_result(ZX_OK);
  });

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(fbl::round_up(kBlockSize, zx_system_get_page_size()), 0, &vmo));
  nand::ReadWriteOperation op = {};
  op.vmo = std::move(vmo);
  op.block = 1;
  op.block_count = 1;

  ASSERT_NO_FATAL_FAILURE(Read(std::move(op)));
  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    ASSERT_EQ(env.bad_block().grown_bad_blocks().size(), 0u);
    ASSERT_EQ(env.nand().last_op(), NAND_OP_READ);
  });
}

TEST_F(SkipBlockTest, ReadMultipleCopiesNoneSucceeds) {
  driver_test().RunInEnvironmentTypeContext([](auto& env) { env.nand().set_block_count(4); });
  StartDriver(4);

  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    // Read Block 0
    env.nand().set_result(ZX_ERR_IO);
    // Read Block 1
    env.nand().set_result(ZX_ERR_IO);
    // Read Block 2
    env.nand().set_result(ZX_ERR_IO);
    // Read Block 3
    env.nand().set_result(ZX_ERR_IO);
  });

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(fbl::round_up(kBlockSize, zx_system_get_page_size()), 0, &vmo));
  nand::ReadWriteOperation op = {};
  op.vmo = std::move(vmo);
  op.block = 0;
  op.block_count = 1;

  ASSERT_NO_FATAL_FAILURE(Read(std::move(op), ZX_ERR_IO));
  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    ASSERT_EQ(env.bad_block().grown_bad_blocks().size(), 0u);
    ASSERT_EQ(env.nand().last_op(), NAND_OP_READ);
  });
}

TEST_F(SkipBlockTest, WriteBytesSingleBlockNoOffset) {
  StartDriver();

  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    // Read block 5.
    env.nand().set_result(ZX_OK);
    // Erase block 5.
    env.nand().set_result(ZX_OK);
    // Write block 5.
    env.nand().set_result(ZX_OK);
  });

  constexpr size_t kSize = kBlockSize - kPageSize;
  constexpr size_t kNandOffset = 5 * kBlockSize;

  zx::vmo vmo;
  ASSERT_NO_FATAL_FAILURE(CreatePayload(kSize, &vmo));
  WriteBytesOperation op = {};
  op.vmo = std::move(vmo);
  op.offset = kNandOffset;
  op.size = kSize;
  op.mode = WriteBytesMode::kReadModifyEraseWrite;

  bool bad_block_grown;
  ASSERT_NO_FATAL_FAILURE(WriteBytes(std::move(op), &bad_block_grown));
  ASSERT_FALSE(bad_block_grown);
  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    ASSERT_EQ(env.bad_block().grown_bad_blocks().size(), 0u);
    ASSERT_EQ(env.nand().last_op(), NAND_OP_WRITE);
  });
  ASSERT_NO_FATAL_FAILURE(ValidateWritten(kNandOffset, kSize));
  ASSERT_NO_FATAL_FAILURE(ValidateUnwritten(kNandOffset + kSize, kPageSize));
}

TEST_F(SkipBlockTest, WriteBytesSingleBlockWithOffset) {
  StartDriver();

  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    // Read block 5.
    env.nand().set_result(ZX_OK);
    // Erase block 5.
    env.nand().set_result(ZX_OK);
    // Write block 5.
    env.nand().set_result(ZX_OK);
  });

  constexpr size_t kOffset = kPageSize;
  constexpr size_t kSize = kBlockSize - (2 * kPageSize);
  constexpr size_t kNandOffset = 5 * kBlockSize;

  zx::vmo vmo;
  ASSERT_NO_FATAL_FAILURE(CreatePayload(kSize, &vmo));
  WriteBytesOperation op = {};
  op.vmo = std::move(vmo);
  op.offset = kNandOffset + kOffset;
  op.size = kSize;
  op.mode = WriteBytesMode::kReadModifyEraseWrite;

  bool bad_block_grown;
  ASSERT_NO_FATAL_FAILURE(WriteBytes(std::move(op), &bad_block_grown));
  ASSERT_FALSE(bad_block_grown);
  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    ASSERT_EQ(env.bad_block().grown_bad_blocks().size(), 0u);
    ASSERT_EQ(env.nand().last_op(), NAND_OP_WRITE);
  });
  ASSERT_NO_FATAL_FAILURE(ValidateUnwritten(kNandOffset, kPageSize));
  ASSERT_NO_FATAL_FAILURE(ValidateWritten(kNandOffset + kOffset, kSize));
  ASSERT_NO_FATAL_FAILURE(ValidateUnwritten(kNandOffset + kOffset + kSize, kPageSize));
}

TEST_F(SkipBlockTest, WriteBytesMultipleBlocks) {
  StartDriver();

  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    // Read block 4.
    env.nand().set_result(ZX_OK);
    // Read block 6.
    env.nand().set_result(ZX_OK);
    // Erase block 4 - 6.
    env.nand().set_result(ZX_OK);
    env.nand().set_result(ZX_OK);
    env.nand().set_result(ZX_OK);
    // Write block  4 - 6.
    env.nand().set_result(ZX_OK);
    env.nand().set_result(ZX_OK);
    env.nand().set_result(ZX_OK);
  });

  constexpr size_t kOffset = kPageSize;
  constexpr size_t kSize = (kBlockSize * 3) - (2 * kPageSize);
  constexpr size_t kNandOffset = 4 * kBlockSize;

  zx::vmo vmo;
  ASSERT_NO_FATAL_FAILURE(CreatePayload(kSize, &vmo));
  WriteBytesOperation op = {};
  op.vmo = std::move(vmo);
  op.offset = kNandOffset + kOffset;
  op.size = kSize;
  op.mode = WriteBytesMode::kReadModifyEraseWrite;

  bool bad_block_grown;
  ASSERT_NO_FATAL_FAILURE(WriteBytes(std::move(op), &bad_block_grown));
  ASSERT_FALSE(bad_block_grown);
  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    ASSERT_EQ(env.bad_block().grown_bad_blocks().size(), 0u);
    ASSERT_EQ(env.nand().last_op(), NAND_OP_WRITE);
  });
  ASSERT_NO_FATAL_FAILURE(ValidateUnwritten(kNandOffset, kPageSize));
  ASSERT_NO_FATAL_FAILURE(ValidateWritten(kNandOffset + kOffset, kSize));
  ASSERT_NO_FATAL_FAILURE(ValidateUnwritten(kNandOffset + kOffset + kSize, kPageSize));
}

TEST_F(SkipBlockTest, WriteBytesAligned) {
  StartDriver();

  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    // Erase block 4 - 5.
    env.nand().set_result(ZX_OK);
    env.nand().set_result(ZX_OK);
    env.nand().set_result(ZX_OK);
    // Write block  4 - 5.
    env.nand().set_result(ZX_OK);
    env.nand().set_result(ZX_OK);
    env.nand().set_result(ZX_OK);
  });

  constexpr size_t kSize = (kBlockSize * 2);
  constexpr size_t kNandOffset = 4 * kBlockSize;

  zx::vmo vmo;
  ASSERT_NO_FATAL_FAILURE(CreatePayload(kSize, &vmo));
  WriteBytesOperation op = {};
  op.vmo = std::move(vmo);
  op.offset = kNandOffset;
  op.size = kSize;
  op.mode = WriteBytesMode::kReadModifyEraseWrite;

  bool bad_block_grown;
  ASSERT_NO_FATAL_FAILURE(WriteBytes(std::move(op), &bad_block_grown));
  ASSERT_FALSE(bad_block_grown);
  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    ASSERT_EQ(env.bad_block().grown_bad_blocks().size(), 0u);
    ASSERT_EQ(env.nand().last_op(), NAND_OP_WRITE);
  });
  ASSERT_NO_FATAL_FAILURE(ValidateWritten(kNandOffset, kSize));
}

TEST_F(SkipBlockTest, WriteBytesWithoutErase) {
  StartDriver();

  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    // Write block 5 directly without erase
    env.nand().set_result(ZX_OK);
  });

  // Write the second page of block 5
  constexpr size_t kSize = kPageSize;
  constexpr size_t kNandOffset = (5 * kBlockSize) + kPageSize;

  zx::vmo vmo;
  ASSERT_NO_FATAL_FAILURE(CreatePayload(kSize, &vmo));
  WriteBytesOperation op = {};
  op.vmo = std::move(vmo);
  op.offset = kNandOffset;
  op.size = kSize;
  // The option doesn't have effect, but we still need to give a valid value.
  op.mode = WriteBytesMode::kReadModifyEraseWrite;

  ASSERT_NO_FATAL_FAILURE(WriteBytesWithoutErase(std::move(op)));
  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    ASSERT_EQ(env.bad_block().grown_bad_blocks().size(), 0u);
    ASSERT_EQ(env.nand().last_op(), NAND_OP_WRITE);
  });
  ASSERT_NO_FATAL_FAILURE(ValidateWritten(kNandOffset, kSize));
}

TEST_F(SkipBlockTest, GrownMultipleBadBlocksWriteBytesWithoutEraseFollowedByWriteBytes) {
  StartDriver();
  // Bad block 5 and 6

  // Writing starting from page 1 in block 5
  constexpr size_t kBlockOffset = 5;
  constexpr size_t kSize = kPageSize;
  constexpr size_t kNandOffset = (kBlockOffset * kBlockSize) + kPageSize;

  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    // Backed up read.
    env.nand().set_result(ZX_OK);
    // Write block 5 directly without erase and fails
    env.nand().set_result(ZX_ERR_IO);
    // Fall back writes.
    // Erase Block 5
    env.nand().set_result(ZX_OK);
    // Write Block 5. But find that it becomes bad.
    env.nand().set_result(ZX_ERR_IO);
    // Erase Block 6. Find it bad as well.
    env.nand().set_result(ZX_ERR_IO);
    // Erase Block 7
    env.nand().set_result(ZX_OK);
    // Write Block 7
    env.nand().set_result(ZX_OK);
  });

  // Test the safe way of using WriteBytesWithoutErase.

  // Backed up the minimal block range that covers the write range.
  // In this case it is block 5.
  zx::vmo vmo_backed_up;
  ASSERT_OK(zx::vmo::create(kBlockSize, 0, &vmo_backed_up));
  {
    zx::vmo dup;
    vmo_backed_up.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup);
    ReadWriteOperation op = {};
    op.vmo = std::move(dup);
    op.block = 5;
    op.block_count = 1;
    ASSERT_NO_FATAL_FAILURE(Read(std::move(op), ZX_OK));
  }

  zx::vmo data;
  ASSERT_NO_FATAL_FAILURE(CreatePayload(kSize, &data));
  fzl::VmoMapper mapper;
  ASSERT_OK(mapper.Map(data, 0, 0, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE));
  // Update the backed up data with the new data.
  vmo_backed_up.write(mapper.start(), kPageSize, kSize);

  // Attempt to write without erase.
  {
    WriteBytesOperation op;
    op.vmo = std::move(data);
    op.offset = kNandOffset;
    op.size = kSize;
    // The option doesn't have effect, but we still need to give a valid value.
    op.mode = WriteBytesMode::kReadModifyEraseWrite;
    ASSERT_NO_FATAL_FAILURE(WriteBytesWithoutErase(std::move(op), ZX_ERR_IO));
  }

  // Fall back writes on the minimal block range.
  {
    WriteBytesOperation op = {};
    op.vmo = std::move(vmo_backed_up);
    op.offset = kBlockOffset * kBlockSize;
    op.size = kBlockSize;
    // The option doesn't have effect, but we still need to give a valid value.
    op.mode = WriteBytesMode::kReadModifyEraseWrite;
    bool bad_block_grown;
    ASSERT_NO_FATAL_FAILURE(WriteBytes(std::move(op), &bad_block_grown));
    ASSERT_TRUE(bad_block_grown);
    driver_test().RunInEnvironmentTypeContext([](auto& env) {
      ASSERT_EQ(env.bad_block().grown_bad_blocks().size(), 2u);
      ASSERT_EQ(env.bad_block().grown_bad_blocks()[0], 5u);
      ASSERT_EQ(env.bad_block().grown_bad_blocks()[1], 6u);
      ASSERT_EQ(env.nand().last_op(), NAND_OP_WRITE);
    });
  }

  // Validate content, expected to be at block 7.
  ASSERT_NO_FATAL_FAILURE(ValidateWritten((7 * kBlockSize) + kPageSize, kSize));
  ASSERT_NO_FATAL_FAILURE(ValidateUnwritten(7 * kBlockSize, kPageSize));
  ASSERT_NO_FATAL_FAILURE(
      ValidateUnwritten((7 * kBlockSize) + (2 * kPageSize), kBlockSize - (2 * kPageSize)));
}

TEST_F(SkipBlockTest, GetPartitionInfo) {
  StartDriver();

  PartitionInfo info;
  ASSERT_NO_FATAL_FAILURE(GetPartitionInfo(&info));
  ASSERT_EQ(info.block_size_bytes, kBlockSize);
  ASSERT_EQ(info.partition_block_count, kNumBlocks);
}

// This test attempts to write 2 copies of a single block to a partition that is 10 blocks wide.
// The copies of logical block 1, start out as block 1 and 6. After erase or write failures, the
// blocks are marked bad, and blocks 2 and 7 become the new "physical" copies of logical block 1.
TEST_F(SkipBlockTest, WriteMultipleCopies) {
  StartDriver(2);

  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    // Erase Block 1
    env.nand().set_result(ZX_OK);
    // Write Block 1
    env.nand().set_result(ZX_ERR_IO);
    // Erase Block 2
    env.nand().set_result(ZX_OK);
    // Write Block 2
    env.nand().set_result(ZX_OK);
    // Erase Block 6
    env.nand().set_result(ZX_ERR_IO);
    // Erase Block 7
    env.nand().set_result(ZX_OK);
    // Write Block 7
    env.nand().set_result(ZX_OK);
  });

  zx::vmo vmo;
  ASSERT_NO_FATAL_FAILURE(CreatePayload(kBlockSize, &vmo));
  ReadWriteOperation op = {};
  op.vmo = std::move(vmo);
  op.block = 1;
  op.block_count = 1;

  bool bad_block_grown;
  ASSERT_NO_FATAL_FAILURE(Write(std::move(op), &bad_block_grown));
  ASSERT_TRUE(bad_block_grown);
  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    ASSERT_EQ(env.bad_block().grown_bad_blocks().size(), 2u);
    ASSERT_EQ(env.bad_block().grown_bad_blocks()[0], 1u);
    ASSERT_EQ(env.bad_block().grown_bad_blocks()[1], 6u);
    ASSERT_EQ(env.nand().last_op(), NAND_OP_WRITE);
  });
  ASSERT_NO_FATAL_FAILURE(ValidateUnwritten(kBlockSize * 1, kBlockSize));
  ASSERT_NO_FATAL_FAILURE(ValidateWritten(kBlockSize * 2, kBlockSize));
  ASSERT_NO_FATAL_FAILURE(ValidateUnwritten(kBlockSize * 6, kBlockSize));
  ASSERT_NO_FATAL_FAILURE(ValidateWritten(kBlockSize * 7, kBlockSize));
}

// This test attempts to write 2 copies of two blocks to a partition that is 10 blocks wide.
// The copies of logical block 1, start out as block 1 and 6. After erase or write failures, the
// blocks are marked bad, and blocks 2 and 7 become the new "physical" copies of logical block 1,
// and 3 and 8 becomes the new "physical" copies of logical block 2.
TEST_F(SkipBlockTest, WriteMultipleCopiesMultipleBlocks) {
  StartDriver(2);

  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    // Erase Block 1.
    env.nand().set_result(ZX_OK);
    // Write Block 1.
    env.nand().set_result(ZX_ERR_IO);
    // Erase Block 2.
    env.nand().set_result(ZX_OK);
    // Write Block 2.
    env.nand().set_result(ZX_OK);
    // Erase Block 3.
    env.nand().set_result(ZX_OK);
    // Write Block 3.
    env.nand().set_result(ZX_OK);
    // Erase Block 6.
    env.nand().set_result(ZX_ERR_IO);
    // Erase Block 7.
    env.nand().set_result(ZX_OK);
    // Write Block 7.
    env.nand().set_result(ZX_OK);
    // Erase Block 8.
    env.nand().set_result(ZX_OK);
    // Write Block 8.
    env.nand().set_result(ZX_OK);
  });

  zx::vmo vmo;
  ASSERT_NO_FATAL_FAILURE(CreatePayload(kBlockSize * 2, &vmo));
  ReadWriteOperation op = {};
  op.vmo = std::move(vmo);
  op.block = 1;
  op.block_count = 2;

  bool bad_block_grown;
  ASSERT_NO_FATAL_FAILURE(Write(std::move(op), &bad_block_grown));
  ASSERT_TRUE(bad_block_grown);
  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    ASSERT_EQ(env.bad_block().grown_bad_blocks().size(), 2u);
    ASSERT_EQ(env.bad_block().grown_bad_blocks()[0], 1u);
    ASSERT_EQ(env.bad_block().grown_bad_blocks()[1], 6u);
    ASSERT_EQ(env.nand().last_op(), NAND_OP_WRITE);
  });
  ASSERT_NO_FATAL_FAILURE(ValidateUnwritten(kBlockSize * 1, kBlockSize));
  ASSERT_NO_FATAL_FAILURE(ValidateWritten(kBlockSize * 2, kBlockSize * 2));
  ASSERT_NO_FATAL_FAILURE(ValidateUnwritten(kBlockSize * 6, kBlockSize));
  ASSERT_NO_FATAL_FAILURE(ValidateWritten(kBlockSize * 7, kBlockSize * 2));
}

// This test attempts to write 4 copies of a single block to a partition that is 4 blocks wide.
// The copies of logical block 0, live in blocks 0, 1, 2, and 3. Since there are no extra copies of
// the blocks, a write/erase failure doesn't result in a new physical block for that copy being
// written. Instead we just continue to next copy. Despite only one copy of the block being written
// successfully, the write request succeeds. We validate all failed blocks are bad blocks grown.
TEST_F(SkipBlockTest, WriteMultipleCopiesOneSucceeds) {
  driver_test().RunInEnvironmentTypeContext([](auto& env) { env.nand().set_block_count(4); });
  StartDriver(4);

  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    // Erase Block 0
    env.nand().set_result(ZX_OK);
    // Write Block 0
    env.nand().set_result(ZX_ERR_IO);
    // Erase Block 1
    env.nand().set_result(ZX_ERR_IO);
    // Erase Block 2
    env.nand().set_result(ZX_OK);
    // Write Block 2
    env.nand().set_result(ZX_OK);
    // Erase Block 3
    env.nand().set_result(ZX_OK);
    // Write Block 3
    env.nand().set_result(ZX_ERR_IO);
  });

  zx::vmo vmo;
  ASSERT_NO_FATAL_FAILURE(CreatePayload(kBlockSize, &vmo));
  ReadWriteOperation op = {};
  op.vmo = std::move(vmo);
  op.block = 0;
  op.block_count = 1;

  bool bad_block_grown;
  ASSERT_NO_FATAL_FAILURE(Write(std::move(op), &bad_block_grown));
  ASSERT_TRUE(bad_block_grown);
  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    ASSERT_EQ(env.bad_block().grown_bad_blocks().size(), 3u);
    ASSERT_EQ(env.bad_block().grown_bad_blocks()[0], 0u);
    ASSERT_EQ(env.bad_block().grown_bad_blocks()[1], 1u);
    ASSERT_EQ(env.bad_block().grown_bad_blocks()[2], 3u);
    ASSERT_EQ(env.nand().last_op(), NAND_OP_WRITE);
  });
  ASSERT_NO_FATAL_FAILURE(ValidateUnwritten(0, kBlockSize * 2));
  ASSERT_NO_FATAL_FAILURE(ValidateWritten(kBlockSize * 2, kBlockSize));
  ASSERT_NO_FATAL_FAILURE(ValidateUnwritten(kBlockSize * 3, kBlockSize));
}

// This test attempts to write 4 copies of a single block. The copies live in blocks 0, 1, 2, and 3.
// The first operation, a block erase, fails for each copy of the block. We validate that the
// overall write also fails and all failed blocks are grown bad blocks.
TEST_F(SkipBlockTest, WriteMultipleCopiesNoneSucceeds) {
  driver_test().RunInEnvironmentTypeContext([](auto& env) { env.nand().set_block_count(4); });
  StartDriver(4);

  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    // Erase Block 0
    env.nand().set_result(ZX_ERR_IO);
    // Erase Block 1
    env.nand().set_result(ZX_ERR_IO);
    // Erase Block 2
    env.nand().set_result(ZX_ERR_IO);
    // Erase Block 3
    env.nand().set_result(ZX_ERR_IO);
  });

  zx::vmo vmo;
  ASSERT_NO_FATAL_FAILURE(CreatePayload(kBlockSize, &vmo));
  ReadWriteOperation op = {};
  op.vmo = std::move(vmo);
  op.block = 0;
  op.block_count = 1;

  bool bad_block_grown;
  ASSERT_NO_FATAL_FAILURE(Write(std::move(op), &bad_block_grown, ZX_ERR_IO));
  ASSERT_TRUE(bad_block_grown);
  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    ASSERT_EQ(env.bad_block().grown_bad_blocks().size(), 4u);
    ASSERT_EQ(env.bad_block().grown_bad_blocks()[0], 0u);
    ASSERT_EQ(env.bad_block().grown_bad_blocks()[1], 1u);
    ASSERT_EQ(env.bad_block().grown_bad_blocks()[2], 2u);
    ASSERT_EQ(env.bad_block().grown_bad_blocks()[3], 3u);
    ASSERT_EQ(env.nand().last_op(), NAND_OP_ERASE);
  });
  ASSERT_NO_FATAL_FAILURE(ValidateUnwritten(0, kBlockSize * 4));
}

}  // namespace

}  // namespace nand::testing
