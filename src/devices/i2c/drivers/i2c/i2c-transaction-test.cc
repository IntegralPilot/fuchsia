// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "src/devices/i2c/drivers/i2c/i2c-test-env.h"

namespace i2c {

namespace {

constexpr uint32_t kTestAddress1 = 5;
constexpr uint32_t kTestAddress2 = 27;
constexpr uint32_t kTestBusId = 6;
const std::string_view kTestChild1Name = "i2c-6-5";
const std::string_view kTestChild2Name = "i2c-6-27";

constexpr uint8_t kTestWrite0 = 0x99;
constexpr uint8_t kTestWrite1 = 0x88;
constexpr uint8_t kTestWrite2 = 0x77;
constexpr uint8_t kTestRead0 = 0x12;
constexpr uint8_t kTestRead1 = 0x34;
constexpr uint8_t kTestRead2 = 0x56;

class I2cDriverTransactionTest : public ::testing::Test {
 protected:
  void Init(FakeI2cImpl::OnTransact on_transact) {
    std::vector<fuchsia_hardware_i2c_businfo::I2CChannel> kChannels = {
        {{
            .address = kTestAddress1,
            .i2c_class = 10,
            .vid = 10,
            .pid = 10,
            .did = 10,
        }},
        {{
            .address = kTestAddress2,
            .i2c_class = 10,
            .vid = 10,
            .pid = 10,
            .did = 10,
        }},
    };

    fuchsia_hardware_i2c_businfo::I2CBusMetadata metadata;
    metadata.channels(kChannels);
    metadata.bus_id(kTestBusId);

    test_runner.RunInEnvironmentTypeContext(
        [on_transact = std::move(on_transact), metadata](TestEnvironment& env) {
          env.AddMetadata(metadata);
          env.i2c_impl().set_on_transact(std::move(on_transact));
        });
    EXPECT_TRUE(test_runner
                    .StartDriverWithCustomStartArgs([](fdf::DriverStartArgs& args) {
                      i2c_config::Config config{{.enable_suspend = true}};
                      args.config(config.ToVmo());
                    })
                    .is_ok());

    zx::result result = test_runner.Connect<fuchsia_hardware_i2c::Service::Device>(kTestChild1Name);
    ASSERT_TRUE(result.is_ok());
    i2c_client.Bind(std::move(result.value()));
    ASSERT_TRUE(i2c_client.is_valid());
  }

  fdf_testing::BackgroundDriverTest<TestConfig> test_runner;
  fidl::WireSyncClient<fuchsia_hardware_i2c::Device> i2c_client;
};

TEST_F(I2cDriverTransactionTest, Write3BytesOnce) {
  fidl::Arena arena;
  Init([](FakeI2cImpl::TransactRequestView req, fdf::Arena& arena,
          FakeI2cImpl::TransactCompleter::Sync& comp) {
    fidl::VectorView<fuchsia_hardware_i2cimpl::wire::I2cImplOp>& ops = req->op;

    if (ops.size() != 1) {
      comp.buffer(arena).ReplyError(ZX_ERR_INTERNAL);
      return;
    }

    const auto& op = ops[0];
    if (op.type.is_read_size()) {
      comp.buffer(arena).ReplyError(ZX_ERR_INTERNAL);
      return;
    }

    auto write_data = op.type.write_data();
    if (write_data.size() != 3 || write_data[0] != kTestWrite0 || write_data[1] != kTestWrite1 ||
        write_data[2] != kTestWrite2) {
      comp.buffer(arena).ReplyError(ZX_ERR_INTERNAL);
      return;
    }

    // No read data.
    comp.buffer(arena).ReplySuccess({});
  });

  // 3 bytes in 1 write transaction.
  size_t n_write_bytes = 3;
  auto write_buffer = std::make_unique<uint8_t[]>(n_write_bytes);
  write_buffer[0] = kTestWrite0;
  write_buffer[1] = kTestWrite1;
  write_buffer[2] = kTestWrite2;
  auto write_data = fidl::VectorView<uint8_t>::FromExternal(write_buffer.get(), n_write_bytes);

  auto write_transfer = fidl_i2c::wire::DataTransfer::WithWriteData(arena, write_data);

  auto transactions = fidl::VectorView<fidl_i2c::wire::Transaction>(arena, 1);
  transactions[0] =
      fidl_i2c::wire::Transaction::Builder(arena).data_transfer(write_transfer).Build();

  auto result = i2c_client->Transfer(transactions);
  ASSERT_OK(result.status());
  ASSERT_FALSE(result->is_error());

  EXPECT_EQ(test_runner.StopDriver().status_value(), ZX_OK);
}

TEST_F(I2cDriverTransactionTest, Read3BytesOnce) {
  fidl::Arena arena;
  Init([](FakeI2cImpl::TransactRequestView req, fdf::Arena& arena,
          FakeI2cImpl::TransactCompleter::Sync& comp) {
    fidl::VectorView<fuchsia_hardware_i2cimpl::wire::I2cImplOp>& ops = req->op;

    if (ops.size() != 1) {
      comp.buffer(arena).ReplyError(ZX_ERR_INTERNAL);
      return;
    }

    const auto& op = ops[0];
    if (op.type.is_write_data() || !op.stop || op.type.read_size() != 3) {
      comp.buffer(arena).ReplyError(ZX_ERR_INTERNAL);
      return;
    }

    std::vector<uint8_t> data{kTestRead0, kTestRead1, kTestRead2};
    fidl::VectorView<fuchsia_hardware_i2cimpl::wire::ReadData> read{arena, 1};
    read[0].data = fidl::VectorView<uint8_t>{arena, data};
    comp.buffer(arena).ReplySuccess(read);
  });

  // 1 read transaction expecting 3 bytes.
  constexpr size_t n_bytes = 3;

  auto read_transfer = fidl_i2c::wire::DataTransfer::WithReadSize(n_bytes);

  auto transactions = fidl::VectorView<fidl_i2c::wire::Transaction>(arena, 1);
  transactions[0] =
      fidl_i2c::wire::Transaction::Builder(arena).data_transfer(read_transfer).Build();

  auto read = i2c_client->Transfer(transactions);
  ASSERT_OK(read.status());
  ASSERT_FALSE(read->is_error());
  ASSERT_EQ(read->value()->read_data.size(), 1u);
  ASSERT_EQ(read->value()->read_data[0][0], kTestRead0);
  ASSERT_EQ(read->value()->read_data[0][1], kTestRead1);
  ASSERT_EQ(read->value()->read_data[0][2], kTestRead2);

  EXPECT_EQ(test_runner.StopDriver().status_value(), ZX_OK);
}

TEST_F(I2cDriverTransactionTest, Write1ByteOnceRead1Byte3TimesTransactions) {
  Init([](FakeI2cImpl::TransactRequestView req, fdf::Arena& arena,
          FakeI2cImpl::TransactCompleter::Sync& comp) {
    fidl::VectorView<fuchsia_hardware_i2cimpl::wire::I2cImplOp>& ops = req->op;

    if (ops.size() != 4) {
      comp.buffer(arena).ReplyError(ZX_ERR_INTERNAL);
      return;
    }

    const auto& op1 = ops[0];
    const auto& op2 = ops[1];
    const auto& op3 = ops[2];
    const auto& op4 = ops[3];

    if (op1.type.is_read_size()) {
      comp.buffer(arena).ReplyError(ZX_ERR_INTERNAL);
      return;
    }
    const auto& op1_write_data = op1.type.write_data();
    if (op1_write_data.size() != 1 || op1_write_data[0] != kTestWrite0) {
      comp.buffer(arena).ReplyError(ZX_ERR_INTERNAL);
      return;
    }

    if (op2.type.is_write_data() || op2.type.read_size() != 1 || op3.type.is_write_data() ||
        op3.type.read_size() != 1 || op4.type.is_write_data() || op4.type.read_size() != 1) {
      comp.buffer(arena).ReplyError(ZX_ERR_INTERNAL);
      return;
    }

    std::vector<uint8_t> data0{kTestRead0};
    std::vector<uint8_t> data1{kTestRead1};
    std::vector<uint8_t> data2{kTestRead2};

    fidl::VectorView<fuchsia_hardware_i2cimpl::wire::ReadData> read{arena, 3};
    read[0].data = fidl::VectorView<uint8_t>{arena, data0};
    read[1].data = fidl::VectorView<uint8_t>{arena, data1};
    read[2].data = fidl::VectorView<uint8_t>{arena, data2};

    comp.buffer(arena).ReplySuccess(read);
  });

  // 1 byte in 1 write transaction.
  size_t n_write_bytes = 1;
  auto write_buffer = std::make_unique<uint8_t[]>(n_write_bytes);
  write_buffer[0] = kTestWrite0;
  auto write_data = fidl::VectorView<uint8_t>::FromExternal(write_buffer.get(), n_write_bytes);

  fidl::Arena arena;
  auto transactions = fidl::VectorView<fidl_i2c::wire::Transaction>(arena, 4);

  transactions[0] =
      fidl_i2c::wire::Transaction::Builder(arena)
          .data_transfer(fidl_i2c::wire::DataTransfer::WithWriteData(arena, write_data))
          .Build();

  // 3 read transaction expecting 1 byte each.
  transactions[1] = fidl_i2c::wire::Transaction::Builder(arena)
                        .data_transfer(fidl_i2c::wire::DataTransfer::WithReadSize(1))
                        .Build();

  transactions[2] = fidl_i2c::wire::Transaction::Builder(arena)
                        .data_transfer(fidl_i2c::wire::DataTransfer::WithReadSize(1))
                        .Build();

  transactions[3] = fidl_i2c::wire::Transaction::Builder(arena)
                        .data_transfer(fidl_i2c::wire::DataTransfer::WithReadSize(1))
                        .Build();

  auto read = i2c_client->Transfer(transactions);
  ASSERT_OK(read.status());
  ASSERT_FALSE(read->is_error());

  ASSERT_EQ(read->value()->read_data[0][0], kTestRead0);
  ASSERT_EQ(read->value()->read_data[1][0], kTestRead1);
  ASSERT_EQ(read->value()->read_data[2][0], kTestRead2);

  EXPECT_EQ(test_runner.StopDriver().status_value(), ZX_OK);
}

TEST_F(I2cDriverTransactionTest, StopFlagPropagates) {
  fidl::Arena arena;
  Init([](FakeI2cImpl::TransactRequestView req, fdf::Arena& arena,
          FakeI2cImpl::TransactCompleter::Sync& comp) {
    fidl::VectorView<fuchsia_hardware_i2cimpl::wire::I2cImplOp>& ops = req->op;
    if (ops.size() != 4) {
      comp.buffer(arena).ReplyError(ZX_ERR_INTERNAL);
      return;
    }

    // Verify that the I2C child driver set the stop flags correctly based on the transaction
    // list passed in below.
    if (!ops[0].stop || ops[1].stop || ops[2].stop || !ops[3].stop) {
      comp.buffer(arena).ReplyError(ZX_ERR_INTERNAL);
      return;
    }

    std::vector<uint8_t> data0{kTestRead0};
    std::vector<uint8_t> data1{kTestRead1};
    std::vector<uint8_t> data2{kTestRead2};
    std::vector<uint8_t> data3{kTestRead0};

    fidl::VectorView<fuchsia_hardware_i2cimpl::wire::ReadData> read{arena, 4};
    read[0].data = fidl::VectorView<uint8_t>{arena, data0};
    read[1].data = fidl::VectorView<uint8_t>{arena, data1};
    read[2].data = fidl::VectorView<uint8_t>{arena, data2};
    read[2].data = fidl::VectorView<uint8_t>{arena, data3};

    comp.buffer(arena).ReplySuccess(read);
  });

  auto transactions = fidl::VectorView<fidl_i2c::wire::Transaction>(arena, 4);

  // Specified and set to true: the stop flag should be set to true.
  transactions[0] = fidl_i2c::wire::Transaction::Builder(arena)
                        .data_transfer(fidl_i2c::wire::DataTransfer::WithReadSize(1))
                        .stop(true)
                        .Build();

  // Specified and set to false: the stop flag should be set to false.
  transactions[1] = fidl_i2c::wire::Transaction::Builder(arena)
                        .data_transfer(fidl_i2c::wire::DataTransfer::WithReadSize(1))
                        .stop(false)
                        .Build();

  // Unspecified: the stop flag should be set to false.
  transactions[2] = fidl_i2c::wire::Transaction::Builder(arena)
                        .data_transfer(fidl_i2c::wire::DataTransfer::WithReadSize(1))
                        .Build();

  // Final transaction: the stop flag should be set to true.
  transactions[3] = fidl_i2c::wire::Transaction::Builder(arena)
                        .data_transfer(fidl_i2c::wire::DataTransfer::WithReadSize(1))
                        .stop(false)
                        .Build();

  auto read = i2c_client->Transfer(transactions);
  ASSERT_OK(read.status());
  ASSERT_FALSE(read.value().is_error());

  EXPECT_EQ(test_runner.StopDriver().status_value(), ZX_OK);
}

TEST_F(I2cDriverTransactionTest, BadTransfers) {
  Init([](FakeI2cImpl::TransactRequestView req, fdf::Arena& arena,
          FakeI2cImpl::TransactCompleter::Sync& comp) {
    // Won't be called into, but in case it is, error out.
    comp.buffer(arena).ReplyError(ZX_ERR_INTERNAL);
  });

  {
    // There must be at least one Transaction.
    fidl::Arena arena;
    auto transactions = fidl::VectorView<fidl_i2c::wire::Transaction>(arena, 0);

    auto read = i2c_client->Transfer(transactions);
    ASSERT_OK(read.status());
    ASSERT_TRUE(read->is_error());
  }

  {
    // Each Transaction must have data_transfer set.
    fidl::Arena arena;
    auto transactions = fidl::VectorView<fidl_i2c::wire::Transaction>(arena, 2);

    transactions[0] = fidl_i2c::wire::Transaction::Builder(arena)
                          .data_transfer(fidl_i2c::wire::DataTransfer::WithReadSize(1))
                          .Build();

    transactions[1] = fidl_i2c::wire::Transaction::Builder(arena).stop(true).Build();

    auto read = i2c_client->Transfer(transactions);
    ASSERT_OK(read.status());
    ASSERT_TRUE(read->is_error());
  }

  {
    // Read transfers must be at least one byte.
    fidl::Arena arena;
    auto transactions = fidl::VectorView<fidl_i2c::wire::Transaction>(arena, 2);

    transactions[0] = fidl_i2c::wire::Transaction::Builder(arena)
                          .data_transfer(fidl_i2c::wire::DataTransfer::WithReadSize(1))
                          .Build();

    transactions[1] = fidl_i2c::wire::Transaction::Builder(arena)
                          .data_transfer(fidl_i2c::wire::DataTransfer::WithReadSize(0))
                          .Build();

    auto read = i2c_client->Transfer(transactions);
    ASSERT_OK(read.status());
    ASSERT_TRUE(read->is_error());
  }

  {
    // Each Transaction must have data_transfer set.
    fidl::Arena arena;
    auto transactions = fidl::VectorView<fidl_i2c::wire::Transaction>(arena, 2);

    auto write0 = fidl::VectorView<uint8_t>(arena, 1);
    write0[0] = 0xff;

    auto write1 = fidl::VectorView<uint8_t>(arena, 0);

    transactions[0] = fidl_i2c::wire::Transaction::Builder(arena)
                          .data_transfer(fidl_i2c::wire::DataTransfer::WithWriteData(arena, write0))
                          .Build();

    transactions[1] = fidl_i2c::wire::Transaction::Builder(arena)
                          .data_transfer(fidl_i2c::wire::DataTransfer::WithWriteData(arena, write1))
                          .Build();

    auto read = i2c_client->Transfer(transactions);
    ASSERT_OK(read.status());
    ASSERT_TRUE(read->is_error());
  }

  EXPECT_EQ(test_runner.StopDriver().status_value(), ZX_OK);
}

TEST_F(I2cDriverTransactionTest, HugeTransfer) {
  Init([](FakeI2cImpl::TransactRequestView req, fdf::Arena& arena,
          FakeI2cImpl::TransactCompleter::Sync& comp) {
    fidl::VectorView<fuchsia_hardware_i2cimpl::wire::I2cImplOp>& ops = req->op;
    constexpr size_t kReadCount = 1024;

    std::vector<fuchsia_hardware_i2cimpl::wire::ReadData> reads;
    for (auto& op : ops) {
      if (op.type.is_read_size() > 0) {
        if (op.type.read_size() != kReadCount) {
          comp.buffer(arena).ReplyError(ZX_ERR_IO);
        }
        fuchsia_hardware_i2cimpl::wire::ReadData read{{arena, kReadCount}};
        memset(read.data.data(), 'r', kReadCount);
        reads.push_back(read);
      } else {
        auto& write_data = op.type.write_data();
        if (std::any_of(write_data.begin(), write_data.end(), [](uint8_t b) { return b != 'w'; })) {
          comp.buffer(arena).ReplyError(ZX_ERR_IO);
          return;
        }
      }
    }
    comp.buffer(arena).ReplySuccess({arena, reads});
  });

  auto write_buffer = std::make_unique<uint8_t[]>(1024);
  auto write_data = fidl::VectorView<uint8_t>::FromExternal(write_buffer.get(), 1024);
  memset(write_data.data(), 'w', write_data.size());

  fidl::Arena arena;
  auto write_transfer = fidl_i2c::wire::DataTransfer::WithWriteData(arena, write_data);
  auto read_transfer = fidl_i2c::wire::DataTransfer::WithReadSize(1024);

  auto transactions = fidl::VectorView<fidl_i2c::wire::Transaction>(arena, 2);
  transactions[0] =
      fidl_i2c::wire::Transaction::Builder(arena).data_transfer(write_transfer).Build();
  transactions[1] =
      fidl_i2c::wire::Transaction::Builder(arena).data_transfer(read_transfer).Build();

  auto read = i2c_client->Transfer(transactions);

  ASSERT_OK(read.status());
  ASSERT_FALSE(read->is_error());

  ASSERT_EQ(read->value()->read_data.size(), 1u);
  ASSERT_EQ(read->value()->read_data[0].size(), 1024u);
  cpp20::span data(read->value()->read_data[0].data(), read->value()->read_data[0].size());
  EXPECT_TRUE(std::all_of(data.begin(), data.end(), [](uint8_t b) { return b == 'r'; }));

  EXPECT_EQ(test_runner.StopDriver().status_value(), ZX_OK);
}

TEST_F(I2cDriverTransactionTest, RequestQueue) {
  struct RequestContext {
    fdf::Arena arena;
    fidl::VectorView<fuchsia_hardware_i2cimpl::wire::I2cImplOp> ops;
    FakeI2cImpl::TransactCompleter::Async completer;
  };

  std::shared_ptr<std::vector<RequestContext>> requests;
  test_runner.RunInEnvironmentTypeContext([&requests](TestEnvironment& env) {
    requests = std::make_shared<std::vector<RequestContext>>();
  });

  Init([requests](FakeI2cImpl::TransactRequestView req, fdf::Arena& arena,
                  FakeI2cImpl::TransactCompleter::Sync& comp) {
    RequestContext context{
        .arena = std::move(arena),
        .ops = req->op,
        .completer = comp.ToAsync(),
    };
    requests->push_back(std::move(context));
  });

  fidl::Client<fidl_i2c::Device> client1, client2;

  {
    zx::result<fidl::ClientEnd<fidl_i2c::Device>> result =
        test_runner.Connect<fidl_i2c::Service::Device>(kTestChild1Name);
    ASSERT_TRUE(result.is_ok());
    client1.Bind(*std::move(result), fdf::Dispatcher::GetCurrent()->async_dispatcher());
  }

  {
    zx::result<fidl::ClientEnd<fidl_i2c::Device>> result =
        test_runner.Connect<fidl_i2c::Service::Device>(kTestChild2Name);
    ASSERT_TRUE(result.is_ok());
    client2.Bind(*std::move(result), fdf::Dispatcher::GetCurrent()->async_dispatcher());
  }

  uint32_t replies = 0;

  {
    fidl_i2c::Transaction write_tx;
    write_tx.data_transfer(fidl_i2c::DataTransfer::WithWriteData({1, 2}));

    std::vector<fidl_i2c::Transaction> transactions;
    transactions.push_back(std::move(write_tx));

    client1->Transfer(std::move(transactions))
        .ThenExactlyOnce([&](fidl::Result<fidl_i2c::Device::Transfer>& result) {
          replies++;
          ASSERT_TRUE(result.is_ok());
          EXPECT_EQ(result->read_data().size(), 0u);
        });

    // Order is not guaranteed across clients, so make a synchronous call to make sure I2C core
    // processed our request before continuing.
    bool get_name_completed = false;
    client1->GetName().ThenExactlyOnce(
        [&](fidl::Result<fidl_i2c::Device::GetName>& result) { get_name_completed = true; });
    test_runner.runtime().RunUntil([&]() { return get_name_completed; });
  }

  {
    fidl_i2c::Transaction write_tx;
    write_tx.data_transfer(fidl_i2c::DataTransfer::WithWriteData({3, 4, 5}));

    fidl_i2c::Transaction read_tx;
    read_tx.data_transfer(fidl_i2c::DataTransfer::WithReadSize(5));

    std::vector<fidl_i2c::Transaction> transactions;
    transactions.push_back(std::move(write_tx));
    transactions.push_back(std::move(read_tx));

    client2->Transfer(std::move(transactions))
        .ThenExactlyOnce([&](fidl::Result<fidl_i2c::Device::Transfer>& result) {
          replies++;
          ASSERT_TRUE(result.is_ok());
          ASSERT_EQ(result->read_data().size(), 1u);
          EXPECT_TRUE(
              std::ranges::equal(result->read_data()[0], std::vector<uint8_t>{6, 7, 8, 9, 10}));
        });

    bool get_name_completed = false;
    client2->GetName().ThenExactlyOnce(
        [&](fidl::Result<fidl_i2c::Device::GetName>& result) { get_name_completed = true; });
    test_runner.runtime().RunUntil([&]() { return get_name_completed; });
  }

  {
    fidl_i2c::Transaction read_tx;
    read_tx.data_transfer(fidl_i2c::DataTransfer::WithReadSize(3));

    std::vector<fidl_i2c::Transaction> transactions;
    transactions.push_back(std::move(read_tx));

    client1->Transfer(std::move(transactions))
        .ThenExactlyOnce([&](fidl::Result<fidl_i2c::Device::Transfer>& result) {
          replies++;
          ASSERT_TRUE(result.is_ok());
          ASSERT_EQ(result->read_data().size(), 1u);
          EXPECT_TRUE(std::ranges::equal(result->read_data()[0], std::vector<uint8_t>{11, 12, 13}));
        });
  }

  // I2C core will make requests to the i2cimpl driver one at a time. Wait for the i2cimpl driver to
  // receive each one.
  for (size_t requests_received = 0; requests_received != 1;) {
    requests_received = test_runner.RunInEnvironmentTypeContext<size_t>(
        [requests](TestEnvironment& env) { return requests->size(); });
  }

  test_runner.RunInEnvironmentTypeContext([requests](TestEnvironment& env) {
    RequestContext& request = requests->at(0);
    ASSERT_EQ(request.ops.size(), 1u);
    EXPECT_EQ(request.ops[0].address, kTestAddress1);
    ASSERT_TRUE(request.ops[0].type.is_write_data());

    EXPECT_TRUE(
        std::ranges::equal(request.ops[0].type.write_data().get(), std::vector<uint8_t>{1, 2}));
    request.completer.buffer(request.arena).ReplySuccess({});
  });

  for (size_t requests_received = 0; requests_received != 2;) {
    requests_received = test_runner.RunInEnvironmentTypeContext<size_t>(
        [requests](TestEnvironment& env) { return requests->size(); });
  }

  test_runner.RunInEnvironmentTypeContext([requests](TestEnvironment& env) {
    RequestContext& request = requests->at(1);
    ASSERT_EQ(request.ops.size(), 2u);
    EXPECT_EQ(request.ops[0].address, kTestAddress2);
    ASSERT_TRUE(request.ops[0].type.is_write_data());
    EXPECT_TRUE(
        std::ranges::equal(request.ops[0].type.write_data().get(), std::vector<uint8_t>{3, 4, 5}));

    EXPECT_EQ(request.ops[1].address, kTestAddress2);
    ASSERT_TRUE(request.ops[1].type.is_read_size());
    EXPECT_EQ(request.ops[1].type.read_size(), 5u);

    fidl::VectorView<fuchsia_hardware_i2cimpl::wire::ReadData> response(request.arena, 1);
    response[0].data = fidl::VectorView<uint8_t>(request.arena, 5);
    response[0].data[0] = 6;
    response[0].data[1] = 7;
    response[0].data[2] = 8;
    response[0].data[3] = 9;
    response[0].data[4] = 10;
    request.completer.buffer(request.arena).ReplySuccess(response);
  });

  for (size_t requests_received = 0; requests_received != 3;) {
    requests_received = test_runner.RunInEnvironmentTypeContext<size_t>(
        [requests](TestEnvironment& env) { return requests->size(); });
  }

  test_runner.RunInEnvironmentTypeContext([requests = std::move(requests)](TestEnvironment& env) {
    RequestContext& request = requests->at(2);
    ASSERT_EQ(request.ops.size(), 1u);
    EXPECT_EQ(request.ops[0].address, kTestAddress1);
    ASSERT_TRUE(request.ops[0].type.is_read_size());
    EXPECT_EQ(request.ops[0].type.read_size(), 3u);

    fidl::VectorView<fuchsia_hardware_i2cimpl::wire::ReadData> response(request.arena, 1);
    response[0].data = fidl::VectorView<uint8_t>(request.arena, 3);
    response[0].data[0] = 11;
    response[0].data[1] = 12;
    response[0].data[2] = 13;
    request.completer.buffer(request.arena).ReplySuccess(response);
  });

  test_runner.runtime().RunUntil([&]() { return replies == 3; });

  EXPECT_EQ(test_runner.StopDriver().status_value(), ZX_OK);
}

TEST_F(I2cDriverTransactionTest, RequestQueueShutdown) {
  std::shared_ptr<std::vector<FakeI2cImpl::TransactCompleter::Async>> completers;
  test_runner.RunInEnvironmentTypeContext([&completers](TestEnvironment& env) {
    completers = std::make_shared<std::vector<FakeI2cImpl::TransactCompleter::Async>>();
  });

  Init([completers](FakeI2cImpl::TransactRequestView req, fdf::Arena& arena,
                    FakeI2cImpl::TransactCompleter::Sync& comp) {
    completers->push_back(comp.ToAsync());
  });

  fidl::Client<fidl_i2c::Device> client;

  {
    zx::result<fidl::ClientEnd<fidl_i2c::Device>> result =
        test_runner.Connect<fidl_i2c::Service::Device>(kTestChild1Name);
    ASSERT_TRUE(result.is_ok());
    client.Bind(*std::move(result), fdf::Dispatcher::GetCurrent()->async_dispatcher());
  }

  uint32_t replies = 0;

  {
    fidl_i2c::Transaction read_tx;
    read_tx.data_transfer(fidl_i2c::DataTransfer::WithReadSize(3));

    std::vector<fidl_i2c::Transaction> transactions;
    transactions.push_back(std::move(read_tx));

    client->Transfer(std::move(transactions))
        .ThenExactlyOnce([&](fidl::Result<fidl_i2c::Device::Transfer>& result) {
          replies++;
          // Canceled by dispatcher shutdown on the async i2cimpl client.
          ASSERT_TRUE(result.is_error());
          EXPECT_TRUE(result.error_value().is_domain_error());
        });
  }

  {
    fidl_i2c::Transaction read_tx;
    read_tx.data_transfer(fidl_i2c::DataTransfer::WithReadSize(3));

    std::vector<fidl_i2c::Transaction> transactions;
    transactions.push_back(std::move(read_tx));

    client->Transfer(std::move(transactions))
        .ThenExactlyOnce([&](fidl::Result<fidl_i2c::Device::Transfer>& result) {
          replies++;
          // Canceled by dispatcher shutdown on the I2C server binding.
          ASSERT_TRUE(result.is_error());
          ASSERT_TRUE(result.error_value().is_framework_error());
          EXPECT_TRUE(result.error_value().framework_error().is_peer_closed());
        });
  }

  {
    fidl_i2c::Transaction read_tx;
    read_tx.data_transfer(fidl_i2c::DataTransfer::WithReadSize(3));

    std::vector<fidl_i2c::Transaction> transactions;
    transactions.push_back(std::move(read_tx));

    client->Transfer(std::move(transactions))
        .ThenExactlyOnce([&](fidl::Result<fidl_i2c::Device::Transfer>& result) {
          replies++;
          // Canceled by dispatcher shutdown on the I2C server binding.
          ASSERT_TRUE(result.is_error());
          ASSERT_TRUE(result.error_value().is_framework_error());
          EXPECT_TRUE(result.error_value().framework_error().is_peer_closed());
        });
  }

  {
    // Make sure all three requests have been processed.
    bool get_name_completed = false;
    client->GetName().ThenExactlyOnce(
        [&](fidl::Result<fidl_i2c::Device::GetName>& result) { get_name_completed = true; });
    test_runner.runtime().RunUntil([&]() { return get_name_completed; });
  }

  EXPECT_EQ(test_runner.StopDriver().status_value(), ZX_OK);

  // New requests are rejected after PrepareStop().
  {
    fidl_i2c::Transaction read_tx;
    read_tx.data_transfer(fidl_i2c::DataTransfer::WithReadSize(3));

    std::vector<fidl_i2c::Transaction> transactions;
    transactions.push_back(std::move(read_tx));

    bool transfer_completed = false;
    client->Transfer(std::move(transactions))
        .ThenExactlyOnce([&](fidl::Result<fidl_i2c::Device::Transfer>& result) {
          ASSERT_TRUE(result.is_error());
          ASSERT_TRUE(result.error_value().is_domain_error());
          EXPECT_EQ(result.error_value().domain_error(), ZX_ERR_CANCELED);
          transfer_completed = true;
        });
    test_runner.runtime().RunUntil([&]() { return transfer_completed; });
  }

  test_runner.ShutdownAndDestroyDriver();

  test_runner.runtime().RunUntil([&]() { return replies == 3; });
}

}  // namespace

}  // namespace i2c
