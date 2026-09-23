// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/fake-i2c/fake-i2c.h>

#include <zxtest/zxtest.h>

#include "args.h"
#include "i2cutil2.h"

namespace {

TEST(I2cArgsTest, TestBadCommand) {
  const char* argv[] = {"i2cutil", "\0", "somepath"};
  int argc = std::size(argv);

  auto parsed = i2cutil::Args::FromArgv(argc, argv);
  ASSERT_NOT_OK(parsed.status_value());
}

TEST(I2cArgsTest, TestPing) {
  const char* argv[] = {"i2cutil", "ping"};
  int argc = std::size(argv);

  auto parsed = i2cutil::Args::FromArgv(argc, argv);
  ASSERT_OK(parsed.status_value());
  EXPECT_EQ(parsed->Op(), i2cutil::I2cOp::Ping);
  EXPECT_EQ(parsed->Transactions().size(), 0);
  EXPECT_EQ(parsed->Path(), "");
}

TEST(I2cArgsTest, TestPingIgnoreTrailing) {
  const char* argv[] = {"i2cutil", "ping", "000", "0x01"};
  int argc = std::size(argv);

  auto parsed = i2cutil::Args::FromArgv(argc, argv);
  ASSERT_OK(parsed.status_value());
  EXPECT_EQ(parsed->Op(), i2cutil::I2cOp::Ping);
  EXPECT_EQ(parsed->Transactions().size(), 0);
  EXPECT_EQ(parsed->Path(), "");
}

TEST(I2cArgsTest, TestHelp) {
  const char* argv[] = {"i2cutil", "help"};
  int argc = std::size(argv);

  auto parsed = i2cutil::Args::FromArgv(argc, argv);
  ASSERT_OK(parsed.status_value());
  EXPECT_EQ(parsed->Op(), i2cutil::I2cOp::Help);
  EXPECT_EQ(parsed->Transactions().size(), 0);
  EXPECT_EQ(parsed->Path(), "");
}

TEST(I2cArgsTest, TestReadPartialPath) {
  const char* argv[] = {"i2cutil", "read", "000", "0x01"};
  int argc = std::size(argv);

  char buffer[32];
  snprintf(buffer, std::size(buffer), i2cutil::kI2cPathFormat, 0);

  auto parsed = i2cutil::Args::FromArgv(argc, argv);
  ASSERT_OK(parsed.status_value());
  EXPECT_EQ(parsed->Op(), i2cutil::I2cOp::Read);
  EXPECT_EQ(parsed->Path(), buffer);
}

TEST(I2cArgsTest, TestWriteFullPath) {
  const char path[] = "/dev/class/i2c/000";
  const char* argv[] = {"i2cutil", "write", path, "0x01"};
  int argc = std::size(argv);

  auto parsed = i2cutil::Args::FromArgv(argc, argv);
  ASSERT_OK(parsed.status_value());
  EXPECT_EQ(parsed->Op(), i2cutil::I2cOp::Write);
  EXPECT_EQ(parsed->Path(), path);
}

TEST(I2cArgsTest, TestRFullPath) {
  /// Make sure that 'r' works as a subcommand just as well as 'read'
  const char path[] = "foobar";
  const char* argv[] = {"i2cutil", "r", path, "0x01"};
  int argc = std::size(argv);

  auto parsed = i2cutil::Args::FromArgv(argc, argv);
  ASSERT_OK(parsed.status_value());
  EXPECT_EQ(parsed->Op(), i2cutil::I2cOp::Read);
  EXPECT_EQ(parsed->Path(), path);
}

TEST(I2cArgsTest, TestInsufficientArgs) {
  const char* argv[] = {"i2cutil"};
  int argc = std::size(argv);

  auto parsed = i2cutil::Args::FromArgv(argc, argv);
  ASSERT_NOT_OK(parsed.status_value());
}

TEST(I2cArgsTest, TestReadSegmentData) {
  const char path[] = "foobar";
  const char* argv[] = {"i2cutil", "read", path, "0xab"};
  int argc = std::size(argv);

  auto parsed = i2cutil::Args::FromArgv(argc, argv);
  ASSERT_OK(parsed.status_value());

  EXPECT_EQ(parsed->Transactions().size(), 2);

  // Write the address.
  EXPECT_EQ(parsed->Transactions().at(0).type, i2cutil::TransactionType::Write);
  ASSERT_EQ(parsed->Transactions().at(0).bytes.size(), 1);
  EXPECT_EQ(parsed->Transactions().at(0).count, 0);
  EXPECT_EQ(parsed->Transactions().at(0).bytes.at(0), 0xab);

  // Read the result back.
  EXPECT_EQ(parsed->Transactions().at(1).type, i2cutil::TransactionType::Read);
  EXPECT_EQ(parsed->Transactions().at(1).bytes.size(), 0);
  EXPECT_EQ(parsed->Transactions().at(1).count, 1);
}

TEST(I2cArgsTest, TestWriteSegmentData) {
  const char path[] = "foobar";
  const char* argv[] = {"i2cutil", "write", path, "0xab"};
  int argc = std::size(argv);

  auto parsed = i2cutil::Args::FromArgv(argc, argv);
  ASSERT_OK(parsed.status_value());

  EXPECT_EQ(parsed->Transactions().size(), 1);
  EXPECT_EQ(parsed->Transactions().at(0).type, i2cutil::TransactionType::Write);
  EXPECT_EQ(parsed->Transactions().at(0).count, 0);
  ASSERT_EQ(parsed->Transactions().at(0).bytes.size(), 1);
  EXPECT_EQ(parsed->Transactions().at(0).bytes.at(0), 0xab);
}

TEST(I2cArgsTest, TestSegmentDataOoB) {
  const char path[] = "foobar";
  const char* argv[] = {"i2cutil", "r", path, "0x100"};  // Byte must be between 0x00 and 0xff
  int argc = std::size(argv);

  auto parsed = i2cutil::Args::FromArgv(argc, argv);
  ASSERT_NOT_OK(parsed.status_value());
}

TEST(I2cArgsTest, TestSegmentDataMultipleBytes) {
  const char path[] = "foobar";
  const char* argv[] = {"i2cutil", "write", path, "0x01", "0x02", "0x03"};
  int argc = std::size(argv);

  auto parsed = i2cutil::Args::FromArgv(argc, argv);
  ASSERT_OK(parsed.status_value());

  // Expect one transaction with 3 bytes.
  EXPECT_EQ(parsed->Transactions().size(), 1);
  EXPECT_EQ(parsed->Transactions().at(0).type, i2cutil::TransactionType::Write);
  ASSERT_EQ(parsed->Transactions().at(0).bytes.size(), 3);
  EXPECT_EQ(parsed->Transactions().at(0).bytes.at(0), 0x01);
  EXPECT_EQ(parsed->Transactions().at(0).bytes.at(1), 0x02);
  EXPECT_EQ(parsed->Transactions().at(0).bytes.at(2), 0x03);
  EXPECT_EQ(parsed->Transactions().at(0).bytes.at(2), 0x03);
  EXPECT_EQ(parsed->Transactions().at(0).count, 0);
}

TEST(I2cArgsTest, TestParseTransaction) {
  const char* argv[] = {"i2cutil", "transact", "somepath", "w", "0x20", "r", "3"};
  int argc = std::size(argv);

  auto parsed = i2cutil::Args::FromArgv(argc, argv);
  ASSERT_OK(parsed.status_value());

  ASSERT_EQ(parsed->Transactions().size(), 2);
  EXPECT_EQ(parsed->Transactions().at(0).type, i2cutil::TransactionType::Write);
  ASSERT_EQ(parsed->Transactions().at(0).bytes.size(), 1);
  EXPECT_EQ(parsed->Transactions().at(0).bytes.at(0), 0x20);

  EXPECT_EQ(parsed->Transactions().at(1).type, i2cutil::TransactionType::Read);
  EXPECT_EQ(parsed->Transactions().at(1).bytes.size(), 0);
  EXPECT_EQ(parsed->Transactions().at(1).count, 3);
}

TEST(I2cArgsTest, TestParseTransactionNoOpcode) {
  const char* argv[] = {"i2cutil", "transact", "somepath",
                        "0x20",    "r",        "3"};  // Transaction must start with 'w' or 'r'
  int argc = std::size(argv);

  auto parsed = i2cutil::Args::FromArgv(argc, argv);
  ASSERT_NOT_OK(parsed.status_value());
}

TEST(I2cArgsTest, TestParseTransactionMultibyte) {
  const char* argv[] = {"i2cutil", "transact", "somepath", "w",
                        "0x01",    "0x02",     "0x03",               // Transaction 1
                        "r",       "5",                              // Transaction 2
                        "w",       "0x07",     "0x08",     "0x09"};  // Transaction 3
  int argc = std::size(argv);

  auto parsed = i2cutil::Args::FromArgv(argc, argv);
  ASSERT_OK(parsed.status_value());

  // Expect 3 transactions with 3 bytes each.
  EXPECT_EQ(parsed->Transactions().size(), 3);

  // Transaction 1
  EXPECT_EQ(parsed->Transactions().at(0).type, i2cutil::TransactionType::Write);
  ASSERT_EQ(parsed->Transactions().at(0).bytes.size(), 3);
  EXPECT_EQ(parsed->Transactions().at(0).bytes.at(0), 0x01);
  EXPECT_EQ(parsed->Transactions().at(0).bytes.at(1), 0x02);
  EXPECT_EQ(parsed->Transactions().at(0).bytes.at(2), 0x03);

  // Transaction 2
  EXPECT_EQ(parsed->Transactions().at(1).type, i2cutil::TransactionType::Read);
  ASSERT_EQ(parsed->Transactions().at(1).bytes.size(), 0);
  ASSERT_EQ(parsed->Transactions().at(1).count, 5);

  // Transaction 3
  EXPECT_EQ(parsed->Transactions().at(2).type, i2cutil::TransactionType::Write);
  ASSERT_EQ(parsed->Transactions().at(2).bytes.size(), 3);
  EXPECT_EQ(parsed->Transactions().at(2).bytes.at(0), 0x07);
  EXPECT_EQ(parsed->Transactions().at(2).bytes.at(1), 0x08);
  EXPECT_EQ(parsed->Transactions().at(2).bytes.at(2), 0x09);
}

TEST(I2cArgsTest, TestParseTransactionBadRead) {
  const char* argv[] = {"i2cutil", "transact", "somepath",
                        "r",       "1",        "2"};  // 'r' must be followed by exactly 1 number
  int argc = std::size(argv);

  auto parsed = i2cutil::Args::FromArgv(argc, argv);
  ASSERT_NOT_OK(parsed.status_value());
}

TEST(I2cArgsTest, TestCreateDumpTransactions) {
  const char* argv[] = {"i2cutil", "dump", "somepath", "0x10", "0x3"};
  int argc = std::size(argv);

  auto parsed = i2cutil::Args::FromArgv(argc, argv);
  ASSERT_OK(parsed.status_value());

  // We asked for count = 0x3 registers which means that args should generate
  // 3 reg * 2 txn/reg = 6txn.
  ASSERT_EQ(parsed->Transactions().size(), 6);
  for (size_t i = 0; i < parsed->Transactions().size(); i++) {
    if (i % 2 == 0) {
      EXPECT_EQ(parsed->Transactions()[i].type, i2cutil::TransactionType::Write);
    } else {
      EXPECT_EQ(parsed->Transactions()[i].type, i2cutil::TransactionType::Read);
    }
  }
}

TEST(I2cArgsTest, TestCreateDumpTransactionsOutOfRange) {
  {
    const char* argv[] = {"i2cutil", "dump", "somepath", "256", "10"};
    int argc = std::size(argv);

    auto parsed = i2cutil::Args::FromArgv(argc, argv);
    EXPECT_NOT_OK(parsed.status_value());
  }
  {
    const char* argv[] = {"i2cutil", "dump", "somepath", "0", "256"};
    int argc = std::size(argv);

    auto parsed = i2cutil::Args::FromArgv(argc, argv);
    EXPECT_NOT_OK(parsed.status_value());
  }
  {
    const char* argv[] = {"i2cutil", "dump", "somepath", "129", "129"};
    int argc = std::size(argv);

    auto parsed = i2cutil::Args::FromArgv(argc, argv);
    EXPECT_NOT_OK(parsed.status_value());
  }
}

class FakeI2cDevice : public fake_i2c::FakeI2c {
 public:
  void PushReadByte(uint8_t byte) { reads_.push_back(byte); }

  const std::vector<uint8_t>& Reads() { return reads_; }
  const std::vector<uint8_t>& Writes() { return writes_; }

 protected:
  zx_status_t Transact(const uint8_t* write_buffer, size_t write_buffer_size, uint8_t* read_buffer,
                       size_t* read_buffer_size, size_t expected_read_size) override {
    writes_.insert(writes_.end(), write_buffer, write_buffer + write_buffer_size);
    assert(reads_.size() <= fuchsia_hardware_i2c::wire::kMaxTransferSize);
    memcpy(read_buffer, reads_.data(), reads_.size());
    *read_buffer_size = reads_.size();

    return ZX_OK;
  }

  std::vector<uint8_t> reads_;
  std::vector<uint8_t> writes_;
};

class I2cUtilTest : public zxtest::Test {
 public:
  void SetUp() override {
    loop_ = std::make_unique<async::Loop>(&kAsyncLoopConfigNeverAttachToThread);

    auto server = fidl::CreateEndpoints(&client_);
    ASSERT_OK(server.status_value());

    fidl::BindServer(loop_->dispatcher(), std::move(server.value()),
                     static_cast<fidl::WireServer<fuchsia_hardware_i2c::Device>*>(&i2c_));

    ASSERT_OK(loop_->StartThread("i2cutil-test-loop"));
  }

  void TearDown() override { loop_->Shutdown(); }

  std::vector<i2cutil::TransactionData> CreateTransactions(size_t n);

 protected:
  std::unique_ptr<async::Loop> loop_;
  fidl::ClientEnd<fuchsia_hardware_i2c::Device> client_;
  FakeI2cDevice i2c_;
};

std::vector<i2cutil::TransactionData> I2cUtilTest::CreateTransactions(size_t n) {
  std::vector<i2cutil::TransactionData> transactions;
  transactions.reserve(n);

  for (size_t i = 0; i < n; i++) {
    i2cutil::TransactionData t;
    t.type = i2cutil::TransactionType::Write;
    t.bytes.push_back(static_cast<uint8_t>(i));
    transactions.push_back(t);
  }

  return transactions;
}

TEST_F(I2cUtilTest, TestWriteBytes) {
  i2cutil::TransactionData t1;
  t1.type = i2cutil::TransactionType::Write;
  t1.bytes.push_back(0x1);
  t1.bytes.push_back(0x2);
  t1.bytes.push_back(0x3);

  std::vector<i2cutil::TransactionData> transactions;
  transactions.push_back(t1);

  i2cutil::execute(std::move(client_), transactions);

  EXPECT_EQ(i2c_.Reads().size(), 0);
  ASSERT_EQ(i2c_.Writes().size(), 3);
  EXPECT_EQ(i2c_.Writes().at(0), 0x1);
  EXPECT_EQ(i2c_.Writes().at(1), 0x2);
  EXPECT_EQ(i2c_.Writes().at(2), 0x3);
}

TEST_F(I2cUtilTest, TestReadBytes) {
  i2cutil::TransactionData t1;
  t1.type = i2cutil::TransactionType::Read;
  t1.count = 3;

  std::vector<i2cutil::TransactionData> transactions;
  transactions.push_back(t1);

  i2c_.PushReadByte(0x01);
  i2c_.PushReadByte(0x02);
  i2c_.PushReadByte(0x03);

  i2cutil::execute(std::move(client_), transactions);

  EXPECT_EQ(i2c_.Writes().size(), 0);
  EXPECT_EQ(transactions.size(), 1);
  ASSERT_EQ(transactions.at(0).bytes.size(), 3);
  EXPECT_EQ(transactions.at(0).bytes.at(0), 0x01);
  EXPECT_EQ(transactions.at(0).bytes.at(1), 0x02);
  EXPECT_EQ(transactions.at(0).bytes.at(2), 0x03);
}

TEST_F(I2cUtilTest, TestMultiTransaction) {
  i2cutil::TransactionData t1;
  t1.type = i2cutil::TransactionType::Write;
  t1.bytes.push_back(0x1);
  t1.bytes.push_back(0x2);
  t1.bytes.push_back(0x3);

  i2c_.PushReadByte(0x4);
  i2c_.PushReadByte(0x5);
  i2c_.PushReadByte(0x6);
  i2cutil::TransactionData t2;
  t2.type = i2cutil::TransactionType::Read;
  t2.count = 3;

  i2cutil::TransactionData t3;
  t3.type = i2cutil::TransactionType::Write;
  t3.bytes.push_back(0x7);
  t3.bytes.push_back(0x8);
  t3.bytes.push_back(0x9);

  std::vector<i2cutil::TransactionData> transactions = {t1, t2, t3};

  i2cutil::execute(std::move(client_), transactions);

  ASSERT_EQ(i2c_.Writes().size(), 6);
  EXPECT_EQ(i2c_.Writes().at(0), 0x1);
  EXPECT_EQ(i2c_.Writes().at(1), 0x2);
  EXPECT_EQ(i2c_.Writes().at(2), 0x3);
  EXPECT_EQ(i2c_.Writes().at(3), 0x7);
  EXPECT_EQ(i2c_.Writes().at(4), 0x8);
  EXPECT_EQ(i2c_.Writes().at(5), 0x9);

  ASSERT_EQ(transactions.size(), 3);
  ASSERT_EQ(transactions.at(1).bytes.size(), 3);
  EXPECT_EQ(transactions.at(1).bytes.at(0), 0x4);
  EXPECT_EQ(transactions.at(1).bytes.at(1), 0x5);
  EXPECT_EQ(transactions.at(1).bytes.at(2), 0x6);
}

TEST_F(I2cUtilTest, TestPaginatedLessThanOnePage) {
  // In this test we want to test that pagination works correctly when there are
  // fewer than one page worth of transactions.
  constexpr size_t kNumTransactions = i2cutil::kMaxTransactionCount - 1;

  std::vector<i2cutil::TransactionData> transactions = CreateTransactions(kNumTransactions);

  i2cutil::execute(std::move(client_), transactions);

  EXPECT_EQ(i2c_.Writes().size(), transactions.size());
}

TEST_F(I2cUtilTest, TestPaginatedExactlyOnePage) {
  // In this test we want to test that pagination works correctly when the number
  // of transactions exactly matches the max allowable number of transactions.
  constexpr size_t kNumTransactions = i2cutil::kMaxTransactionCount;

  std::vector<i2cutil::TransactionData> transactions = CreateTransactions(kNumTransactions);

  i2cutil::execute(std::move(client_), transactions);

  EXPECT_EQ(i2c_.Writes().size(), transactions.size());
}

TEST_F(I2cUtilTest, TestPaginatedMoreThanOnePage) {
  // In this test we want to test that pagination works correctly when there are
  // more than one page worth of transactions.
  constexpr size_t kNumTransactions = i2cutil::kMaxTransactionCount + 1;

  std::vector<i2cutil::TransactionData> transactions = CreateTransactions(kNumTransactions);

  i2cutil::execute(std::move(client_), transactions);

  EXPECT_EQ(i2c_.Writes().size(), transactions.size());
}

}  // namespace
