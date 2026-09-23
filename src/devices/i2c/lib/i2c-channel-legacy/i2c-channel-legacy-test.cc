// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/fake-i2c/fake-i2c.h>
#include <zircon/errors.h>

#include <algorithm>
#include <numeric>
#include <vector>

#include <zxtest/zxtest.h>

#include "src/devices/i2c/lib/i2c-channel-legacy/i2c-channel.h"
#include "src/devices/testing/mock-ddk/mock-device.h"

namespace {

// An I2C device that requires retries.
class FlakyI2cDevice : public fake_i2c::FakeI2c {
 protected:
  zx_status_t Transact(const uint8_t* write_buffer, size_t write_buffer_size, uint8_t* read_buffer,
                       size_t* read_buffer_size, size_t expected_read_size) override {
    count_++;
    // Unique errors below to check for retries.
    switch (count_) {
        // clang-format off
      case 1: return ZX_ERR_INTERNAL; break;
      case 2: return ZX_ERR_NOT_SUPPORTED; break;
      case 3: return ZX_ERR_NO_RESOURCES; break;
      case 4: return ZX_ERR_NO_MEMORY; break;
      case 5: *read_buffer_size = 1; return ZX_OK; break;
      default: ZX_ASSERT(0);  // Anything else is an error.
        // clang-format on
    }
    return ZX_OK;
  }

 private:
  size_t count_ = 0;
};

class I2cDevice : public fidl::WireServer<fuchsia_hardware_i2c::Device> {
 public:
  void Transfer(TransferRequestView request, TransferCompleter::Sync& completer) override {
    const fidl::VectorView<fuchsia_hardware_i2c::wire::Transaction> transactions =
        request->transactions;

    if (std::any_of(transactions.cbegin(), transactions.cend(),
                    [](const fuchsia_hardware_i2c::wire::Transaction& t) {
                      return !t.has_data_transfer();
                    })) {
      completer.ReplyError(ZX_ERR_INVALID_ARGS);
      return;
    }

    fidl::Arena arena;
    fidl::ObjectView<fuchsia_hardware_i2c::wire::DeviceTransferResponse> response(arena);

    stop_.reserve(transactions.size());

    const size_t write_size = std::accumulate(
        transactions.cbegin(), transactions.cend(), 0,
        [](size_t a, const fuchsia_hardware_i2c::wire::Transaction& b) {
          return a +
                 (b.data_transfer().is_write_data() ? b.data_transfer().write_data().size() : 0);
        });
    tx_data_.resize(write_size);

    const size_t read_count = std::count_if(transactions.cbegin(), transactions.cend(),
                                            [](const fuchsia_hardware_i2c::wire::Transaction& t) {
                                              return t.data_transfer().is_read_size();
                                            });
    response->read_data = {arena, read_count};

    size_t tx_offset = 0;
    size_t rx_transaction = 0;
    size_t rx_offset = 0;
    auto stop_it = stop_.begin();
    for (const auto& transaction : transactions) {
      if (transaction.data_transfer().is_read_size()) {
        // If this is a write/read, pass back all of the expected read data, regardless of how much
        // the client requested. This allows the truncation behavior to be tested.
        const size_t read_size =
            read_count == 1 ? rx_data_.size() : transaction.data_transfer().read_size();

        // Copy the expected RX data to each read transaction.
        auto& read_data = response->read_data[rx_transaction++];
        read_data = {arena, read_size};

        if (rx_offset + read_data.size() > rx_data_.size()) {
          completer.ReplyError(ZX_ERR_OUT_OF_RANGE);
          return;
        }

        memcpy(read_data.data(), &rx_data_[rx_offset], read_data.size());
        rx_offset += transaction.data_transfer().read_size();
      } else {
        // Serialize and store the write transaction.
        const auto& write_data = transaction.data_transfer().write_data();
        memcpy(&tx_data_[tx_offset], write_data.data(), write_data.size());
        tx_offset += write_data.size();
      }

      *stop_it++ = transaction.has_stop() ? transaction.stop() : false;
    }

    completer.Reply(::fit::ok(response.get()));
  }

  void GetName(GetNameCompleter::Sync& completer) override {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }

  const std::vector<uint8_t>& tx_data() const { return tx_data_; }
  void set_rx_data(std::vector<uint8_t> rx_data) { rx_data_ = std::move(rx_data); }
  const std::vector<bool>& stop() const { return stop_; }

 private:
  std::vector<uint8_t> tx_data_;
  std::vector<uint8_t> rx_data_;
  std::vector<bool> stop_;
};

template <typename Device>
class I2cServer {
 public:
  explicit I2cServer(async_dispatcher_t* dispatcher)
      : dispatcher_(dispatcher), outgoing_(dispatcher) {}

  void BindProtocol(fidl::ServerEnd<fuchsia_hardware_i2c::Device> server_end) {
    bindings_.AddBinding(dispatcher_, std::move(server_end), &i2c_dev_,
                         fidl::kIgnoreBindingClosure);
  }

  void PublishService(fidl::ServerEnd<fuchsia_io::Directory> directory) {
    zx::result service_result = outgoing_.AddService<fuchsia_hardware_i2c::Service>(
        fuchsia_hardware_i2c::Service::InstanceHandler({
            .device = bindings_.CreateHandler(&i2c_dev_, dispatcher_, fidl::kIgnoreBindingClosure),
        }));
    ASSERT_OK(service_result.status_value());
    ASSERT_OK(outgoing_.Serve(std::move(directory)));
  }

  Device& i2c_dev() { return i2c_dev_; }

 private:
  async_dispatcher_t* dispatcher_;
  component::OutgoingDirectory outgoing_;
  fidl::ServerBindingGroup<fuchsia_hardware_i2c::Device> bindings_;
  Device i2c_dev_;
};

class I2cFlakyChannelTest : public zxtest::Test {
 public:
  using Server = I2cServer<FlakyI2cDevice>;

  I2cFlakyChannelTest() : loop_(&kAsyncLoopConfigNeverAttachToThread) { loop_.StartThread(); }

  void SetUp() final {
    auto endpoints = fidl::Endpoints<fuchsia_hardware_i2c::Device>::Create();
    i2c_server_.AsyncCall(&Server::BindProtocol, std::move(endpoints.server));
    i2c_channel_.emplace(std::move(endpoints.client));
  }

  ddk::I2cChannel& i2c_channel() { return i2c_channel_.value(); }

  async_patterns::TestDispatcherBound<Server>& i2c_server() { return i2c_server_; }

 private:
  async::Loop loop_;
  async_patterns::TestDispatcherBound<Server> i2c_server_{loop_.dispatcher(), std::in_place,
                                                          async_patterns::PassDispatcher};
  std::optional<ddk::I2cChannel> i2c_channel_;
};

TEST_F(I2cFlakyChannelTest, NoRetries) {
  // No retry, the first error is returned.
  uint8_t buffer[1] = {0x12};
  constexpr uint8_t kNumberOfRetries = 0;
  auto ret = i2c_channel().WriteSyncRetries(buffer, sizeof(buffer), kNumberOfRetries, zx::usec(1));
  EXPECT_EQ(ret.status, ZX_ERR_INTERNAL);
  EXPECT_EQ(ret.retries, 0);
}

TEST_F(I2cFlakyChannelTest, RetriesAllFail) {
  // 2 retries, corresponding error is returned. The first time Transact is called we get a
  // ZX_ERR_INTERNAL. Then the first retry gives us ZX_ERR_NOT_SUPPORTED and then the second
  // gives us ZX_ERR_NO_RESOURCES.
  constexpr uint8_t kNumberOfRetries = 2;
  uint8_t buffer[1] = {0x34};
  auto ret =
      i2c_channel().ReadSyncRetries(0x56, buffer, sizeof(buffer), kNumberOfRetries, zx::usec(1));
  EXPECT_EQ(ret.status, ZX_ERR_NO_RESOURCES);
  EXPECT_EQ(ret.retries, 2);
}

TEST_F(I2cFlakyChannelTest, RetriesOk) {
  // 4 retries requested but no error, return ok.
  uint8_t tx_buffer[1] = {0x78};
  uint8_t rx_buffer[1] = {0x90};
  constexpr uint8_t kNumberOfRetries = 5;
  auto ret = i2c_channel().WriteReadSyncRetries(tx_buffer, sizeof(tx_buffer), rx_buffer,
                                                sizeof(rx_buffer), kNumberOfRetries, zx::usec(1));
  EXPECT_EQ(ret.status, ZX_OK);
  EXPECT_EQ(ret.retries, 4);
}

class I2cChannelTest : public zxtest::Test {
 public:
  using Server = I2cServer<I2cDevice>;

  I2cChannelTest() : loop_(&kAsyncLoopConfigNeverAttachToThread) { loop_.StartThread(); }

  void SetUp() final {
    auto endpoints = fidl::Endpoints<fuchsia_hardware_i2c::Device>::Create();
    i2c_server_.AsyncCall(&Server::BindProtocol, std::move(endpoints.server));
    i2c_channel_.emplace(std::move(endpoints.client));
  }

  ddk::I2cChannel& i2c_channel() { return i2c_channel_.value(); }

  async_patterns::TestDispatcherBound<Server>& i2c_server() { return i2c_server_; }

 private:
  async::Loop loop_;
  async_patterns::TestDispatcherBound<Server> i2c_server_{loop_.dispatcher(), std::in_place,
                                                          async_patterns::PassDispatcher};
  std::optional<ddk::I2cChannel> i2c_channel_;
};

TEST_F(I2cChannelTest, FidlRead) {
  static constexpr std::array<uint8_t, 4> expected_rx_data{0x12, 0x34, 0xab, 0xcd};

  i2c_server().SyncCall([](Server* server) {
    server->i2c_dev().set_rx_data(
        {expected_rx_data.data(), expected_rx_data.data() + expected_rx_data.size()});
  });

  uint8_t buf[4];
  EXPECT_OK(i2c_channel().ReadSync(0x89, buf, expected_rx_data.size()));
  i2c_server().SyncCall([](Server* server) {
    ASSERT_EQ(server->i2c_dev().tx_data().size(), 1);
    EXPECT_EQ(server->i2c_dev().tx_data()[0], 0x89);
  });
  EXPECT_BYTES_EQ(buf, expected_rx_data.data(), expected_rx_data.size());

  uint8_t buf1[5];
  // I2cChannel will copy as much data as it receives, even if it is less than the amount requested.
  EXPECT_OK(i2c_channel().ReadSync(0x98, buf1, sizeof(buf1)));
  i2c_server().SyncCall([](Server* server) {
    ASSERT_EQ(server->i2c_dev().tx_data().size(), 1);
    EXPECT_EQ(server->i2c_dev().tx_data()[0], 0x98);
  });
  EXPECT_BYTES_EQ(buf1, expected_rx_data.data(), expected_rx_data.size());

  uint8_t buf2[3];
  // I2cChannel will copy no more than the amount requested.
  EXPECT_OK(i2c_channel().ReadSync(0x18, buf2, sizeof(buf2)));
  i2c_server().SyncCall([](Server* server) {
    ASSERT_EQ(server->i2c_dev().tx_data().size(), 1);
    EXPECT_EQ(server->i2c_dev().tx_data()[0], 0x18);
  });
  EXPECT_BYTES_EQ(buf2, expected_rx_data.data(), sizeof(buf2));
}

TEST_F(I2cChannelTest, FidlWrite) {
  static constexpr std::array<uint8_t, 4> expected_tx_data{0x0f, 0x1e, 0x2d, 0x3c};

  EXPECT_OK(i2c_channel().WriteSync(expected_tx_data.data(), expected_tx_data.size()));

  i2c_server().SyncCall([](Server* server) {
    ASSERT_EQ(server->i2c_dev().tx_data().size(), expected_tx_data.size());
    EXPECT_BYTES_EQ(server->i2c_dev().tx_data().data(), expected_tx_data.data(),
                    expected_tx_data.size());
  });
}

TEST_F(I2cChannelTest, FidlWriteRead) {
  static constexpr std::array<uint8_t, 4> expected_rx_data{0x12, 0x34, 0xab, 0xcd};
  static constexpr std::array<uint8_t, 4> expected_tx_data{0x0f, 0x1e, 0x2d, 0x3c};

  i2c_server().SyncCall([](Server* server) {
    server->i2c_dev().set_rx_data(
        {expected_rx_data.data(), expected_rx_data.data() + expected_rx_data.size()});
  });

  uint8_t buf[4];
  EXPECT_OK(i2c_channel().WriteReadSync(expected_tx_data.data(), expected_tx_data.size(), buf,
                                        expected_rx_data.size()));
  i2c_server().SyncCall([](Server* server) {
    ASSERT_EQ(server->i2c_dev().tx_data().size(), expected_tx_data.size());
    EXPECT_BYTES_EQ(server->i2c_dev().tx_data().data(), expected_tx_data.data(),
                    expected_tx_data.size());
  });
  EXPECT_BYTES_EQ(buf, expected_rx_data.data(), expected_rx_data.size());

  uint8_t buf1[5];
  EXPECT_OK(i2c_channel().WriteReadSync(expected_tx_data.data(), expected_tx_data.size(), buf1,
                                        sizeof(buf1)));
  i2c_server().SyncCall([](Server* server) {
    EXPECT_BYTES_EQ(server->i2c_dev().tx_data().data(), expected_tx_data.data(),
                    expected_tx_data.size());
  });
  EXPECT_BYTES_EQ(buf1, expected_rx_data.data(), expected_rx_data.size());

  uint8_t buf2[3];
  EXPECT_OK(i2c_channel().WriteReadSync(expected_tx_data.data(), expected_tx_data.size(), buf2,
                                        sizeof(buf2)));
  i2c_server().SyncCall([](Server* server) {
    EXPECT_BYTES_EQ(server->i2c_dev().tx_data().data(), expected_tx_data.data(),
                    expected_tx_data.size());
  });
  EXPECT_BYTES_EQ(buf2, expected_rx_data.data(), sizeof(buf2));

  EXPECT_OK(i2c_channel().WriteReadSync(nullptr, 0, buf, expected_rx_data.size()));
  i2c_server().SyncCall([](Server* server) { EXPECT_EQ(server->i2c_dev().tx_data().size(), 0); });
  EXPECT_BYTES_EQ(buf, expected_rx_data.data(), expected_rx_data.size());
}

class I2cChannelServiceTest : public zxtest::Test {
 public:
  using Server = I2cServer<I2cDevice>;

  I2cChannelServiceTest() : loop_(&kAsyncLoopConfigNeverAttachToThread) { loop_.StartThread(); }

  void SetUp() final {
    auto endpoints = fidl::Endpoints<fuchsia_io::Directory>::Create();
    i2c_server_.AsyncCall(&Server::PublishService, std::move(endpoints.server));
    directory_ = std::move(endpoints.client);
  }

  async_patterns::TestDispatcherBound<Server>& i2c_server() { return i2c_server_; }

  fidl::ClientEnd<fuchsia_io::Directory> TakeDirectory() { return std::move(directory_); }

 private:
  async::Loop loop_;
  async_patterns::TestDispatcherBound<Server> i2c_server_{loop_.dispatcher(), std::in_place,
                                                          async_patterns::PassDispatcher};
  fidl::ClientEnd<fuchsia_io::Directory> directory_;
};

TEST_F(I2cChannelServiceTest, GetFidlProtocolFromParent) {
  auto parent = MockDevice::FakeRootParent();
  parent->AddFidlService(fuchsia_hardware_i2c::Service::Name, TakeDirectory());
  ddk::I2cChannel i2c_channel{parent.get()};
  ASSERT_TRUE(i2c_channel.is_valid());

  // Issue a simple call to make sure the connection is working.
  i2c_server().SyncCall([](Server* server) { server->i2c_dev().set_rx_data({0xab}); });

  uint8_t rx;
  EXPECT_OK(i2c_channel.ReadSync(0x89, &rx, 1));
  i2c_server().SyncCall([](Server* server) {
    ASSERT_EQ(server->i2c_dev().tx_data().size(), 1);
    EXPECT_EQ(server->i2c_dev().tx_data()[0], 0x89);
  });
  EXPECT_EQ(rx, 0xab);

  // Move the client and verify that the new one is functional.
  ddk::I2cChannel new_client = std::move(i2c_channel);

  i2c_server().SyncCall([](Server* server) { server->i2c_dev().set_rx_data({0x12}); });
  EXPECT_OK(new_client.ReadSync(0x34, &rx, 1));
  i2c_server().SyncCall([](Server* server) {
    ASSERT_EQ(server->i2c_dev().tx_data().size(), 1);
    EXPECT_EQ(server->i2c_dev().tx_data()[0], 0x34);
  });
  EXPECT_EQ(rx, 0x12);
}

TEST_F(I2cChannelServiceTest, GetFidlProtocolFromFragment) {
  auto parent = MockDevice::FakeRootParent();
  parent->AddFidlService(fuchsia_hardware_i2c::Service::Name, TakeDirectory(), "fragment-name");
  ddk::I2cChannel i2c_channel{parent.get(), "fragment-name"};
  ASSERT_TRUE(i2c_channel.is_valid());

  i2c_server().SyncCall([](Server* server) { server->i2c_dev().set_rx_data({0x56}); });

  uint8_t rx;
  EXPECT_OK(i2c_channel.ReadSync(0x78, &rx, 1));
  i2c_server().SyncCall([](Server* server) {
    ASSERT_EQ(server->i2c_dev().tx_data().size(), 1);
    EXPECT_EQ(server->i2c_dev().tx_data()[0], 0x78);
  });
  EXPECT_EQ(rx, 0x56);
}

}  // namespace
