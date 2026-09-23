// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/i2c/lib/i2c-channel/i2c-channel.h"

#include <lib/async-loop/cpp/loop.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/driver/incoming/cpp/namespace.h>
#include <lib/fake-i2c/fake-i2c.h>
#include <zircon/errors.h>

#include <algorithm>
#include <numeric>
#include <vector>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"

namespace i2c {

// An I2C device that requires retry_count.
class FlakyI2cDevice : public fake_i2c::FakeI2c {
 protected:
  zx_status_t Transact(const uint8_t* write_buffer, size_t write_buffer_size, uint8_t* read_buffer,
                       size_t* read_buffer_size, size_t expected_read_size) override {
    count_++;
    // Unique errors below to check for retry_count.
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
  explicit I2cServer(async_dispatcher_t* dispatcher) : dispatcher_(dispatcher) {}

  void BindProtocol(fidl::ServerEnd<fuchsia_hardware_i2c::Device> server_end) {
    bindings_.AddBinding(dispatcher_, std::move(server_end), &i2c_dev_,
                         fidl::kIgnoreBindingClosure);
  }

  fuchsia_hardware_i2c::Service::InstanceHandler GetInstanceHandler() {
    return fuchsia_hardware_i2c::Service::InstanceHandler{
        {.device = bindings_.CreateHandler(&i2c_dev_, dispatcher_, fidl::kIgnoreBindingClosure)}};
  }

  Device& i2c_dev() { return i2c_dev_; }

 private:
  async_dispatcher_t* dispatcher_;
  fidl::ServerBindingGroup<fuchsia_hardware_i2c::Device> bindings_;
  Device i2c_dev_;
};

class I2cFlakyChannelTest : public ::testing::Test {
 public:
  using Server = I2cServer<FlakyI2cDevice>;

  I2cFlakyChannelTest() : loop_(&kAsyncLoopConfigNeverAttachToThread) { loop_.StartThread(); }

  void SetUp() final {
    auto endpoints = fidl::Endpoints<fuchsia_hardware_i2c::Device>::Create();
    i2c_server_.AsyncCall(&Server::BindProtocol, std::move(endpoints.server));
    i2c_channel_.emplace(std::move(endpoints.client));
  }

  I2cChannel& i2c_channel() { return i2c_channel_.value(); }

  async_patterns::TestDispatcherBound<Server>& i2c_server() { return i2c_server_; }

 private:
  async::Loop loop_;
  async_patterns::TestDispatcherBound<Server> i2c_server_{loop_.dispatcher(), std::in_place,
                                                          async_patterns::PassDispatcher};
  std::optional<I2cChannel> i2c_channel_;
};

TEST_F(I2cFlakyChannelTest, NoRetries) {
  // No retry, the first error is returned.
  static constexpr std::array<uint8_t, 1> kWriteData = {0x12};
  constexpr uint8_t kNumberOfRetries = 0;
  auto ret = i2c_channel().WriteSyncRetries(kWriteData, kNumberOfRetries, zx::usec(1));
  EXPECT_EQ(ret.status_value(), ZX_ERR_INTERNAL);
  EXPECT_EQ(ret.retry_count(), 0u);
}

TEST_F(I2cFlakyChannelTest, RetriesAllFail) {
  // 2 retry_count, corresponding error is returned. The first time Transact is called we get a
  // ZX_ERR_INTERNAL. Then the first retry gives us ZX_ERR_NOT_SUPPORTED and then the second
  // gives us ZX_ERR_NO_RESOURCES.
  constexpr uint8_t kNumberOfRetries = 2;
  std::array<uint8_t, 1> read_data{0x34};
  auto ret = i2c_channel().ReadSyncRetries(0x56, read_data, kNumberOfRetries, zx::usec(1));
  EXPECT_EQ(ret.status_value(), ZX_ERR_NO_RESOURCES);
  EXPECT_EQ(ret.retry_count(), 2u);
}

TEST_F(I2cFlakyChannelTest, RetriesOk) {
  // 4 retry_count requested but no error, return ok.
  static constexpr std::array<uint8_t, 1> kWriteData = {0x78};
  std::array<uint8_t, 1> read_data{0x90};
  constexpr uint8_t kNumberOfRetries = 5;
  auto ret =
      i2c_channel().WriteReadSyncRetries(kWriteData, read_data, kNumberOfRetries, zx::usec(1));
  EXPECT_OK(ret.status_value());
  EXPECT_EQ(ret.retry_count(), 4u);
}

class I2cChannelTest : public ::testing::Test {
 public:
  using Server = I2cServer<I2cDevice>;

  I2cChannelTest() : loop_(&kAsyncLoopConfigNeverAttachToThread) { loop_.StartThread(); }

  void SetUp() final {
    auto endpoints = fidl::Endpoints<fuchsia_hardware_i2c::Device>::Create();
    i2c_server_.AsyncCall(&Server::BindProtocol, std::move(endpoints.server));
    i2c_channel_.emplace(std::move(endpoints.client));
  }

  I2cChannel& i2c_channel() { return i2c_channel_.value(); }

  async_patterns::TestDispatcherBound<Server>& i2c_server() { return i2c_server_; }

 private:
  async::Loop loop_;
  async_patterns::TestDispatcherBound<Server> i2c_server_{loop_.dispatcher(), std::in_place,
                                                          async_patterns::PassDispatcher};
  std::optional<I2cChannel> i2c_channel_;
};

// Verify that `I2cChannel::Read()` writes the correct address and reads the correct data.
TEST_F(I2cChannelTest, Read) {
  static constexpr std::array<uint8_t, 4> kExpectedRxData{0x12, 0x34, 0xab, 0xcd};
  static constexpr uint8_t kExpectedAddress = 0x89;

  i2c_server().SyncCall([](Server* server) {
    server->i2c_dev().set_rx_data(
        {kExpectedRxData.data(), kExpectedRxData.data() + kExpectedRxData.size()});
  });

  // Make sure that `read_data` has enough space to read all of the data.
  std::array<uint8_t, 4> read_data;
  EXPECT_EQ(read_data.size(), kExpectedRxData.size());

  zx::result result = i2c_channel().ReadSync(kExpectedAddress, read_data);
  ASSERT_OK(result);

  // Verify that the correct bytes were read.
  EXPECT_EQ(result.value(), kExpectedRxData.size());
  EXPECT_EQ(read_data, kExpectedRxData);

  // Verify that the address was written to the i2c.
  i2c_server().SyncCall([](Server* server) {
    EXPECT_THAT(server->i2c_dev().tx_data(),
                ::testing::ElementsAreArray({static_cast<unsigned char>(kExpectedAddress)}));
  });
}

// Verify that `I2cChannel::Read()` returns the correct number of bytes read even if the
// provided read buffer is larger than what can be read.
TEST_F(I2cChannelTest, ReadLargeBuffer) {
  static constexpr std::array<uint8_t, 4> kExpectedRxData{0x12, 0x34, 0xab, 0xcd};

  i2c_server().SyncCall([](Server* server) {
    server->i2c_dev().set_rx_data(
        {kExpectedRxData.data(), kExpectedRxData.data() + kExpectedRxData.size()});
  });

  // Purposefully make `large_read_data` larger than what can be read.
  std::array<uint8_t, 5> large_read_data;
  EXPECT_GT(large_read_data.size(), kExpectedRxData.size());

  zx::result result = i2c_channel().ReadSync(0x01, large_read_data);
  ASSERT_OK(result);

  // Verify that the correct number of bytes were read despite `large_read_data.size()` being larger
  // than what can be read.
  EXPECT_EQ(result.value(), kExpectedRxData.size());

  // Verify that the correct number of bytes were read.
  EXPECT_THAT(std::span(large_read_data.begin(), 4), ::testing::ElementsAreArray(kExpectedRxData));
}

// Verify that `I2cChannel::Read()` returns the correct number of bytes read even if the
// provided read buffer is smaller than what can be read.
TEST_F(I2cChannelTest, ReadSmallBuffer) {
  static constexpr std::array<uint8_t, 4> kExpectedRxData{0x12, 0x34, 0xab, 0xcd};

  i2c_server().SyncCall([](Server* server) {
    server->i2c_dev().set_rx_data(
        {kExpectedRxData.data(), kExpectedRxData.data() + kExpectedRxData.size()});
  });

  // Purposefully make `small_read_data` smaller than what can be read.
  std::array<uint8_t, 3> small_read_data;
  EXPECT_LT(small_read_data.size(), kExpectedRxData.size());

  zx::result result = i2c_channel().ReadSync(0x01, small_read_data);
  ASSERT_OK(result);

  // Verify that only `small_read_data.size()` bytes were read even though there is more bytes that
  // can be read.
  EXPECT_EQ(result.value(), small_read_data.size());

  // Verify that the correct number of bytes were read.
  EXPECT_THAT(small_read_data, ::testing::ElementsAreArray(std::span{kExpectedRxData.begin(), 3}));
}

// Verify that `I2cChannel::Write()` writes the correct data to the i2c server.
TEST_F(I2cChannelTest, Write) {
  static constexpr std::array<uint8_t, 4> kExpectedTxData{0x0f, 0x1e, 0x2d, 0x3c};

  EXPECT_OK(i2c_channel().WriteSync(kExpectedTxData));

  i2c_server().SyncCall([](Server* server) {
    EXPECT_THAT(server->i2c_dev().tx_data(), ::testing::ElementsAreArray(kExpectedTxData));
  });
}

// Verify that `I2cChannel::WriteReadSync()` writes the correct data and reads the correct data.
TEST_F(I2cChannelTest, WriteRead) {
  static constexpr std::array<uint8_t, 4> kExpectedRxData{0x12, 0x34, 0xab, 0xcd};
  static constexpr std::array<uint8_t, 4> kExpectedTxData{0x0f, 0x1e, 0x2d, 0x3c};

  i2c_server().SyncCall([](Server* server) {
    server->i2c_dev().set_rx_data(
        {kExpectedRxData.data(), kExpectedRxData.data() + kExpectedRxData.size()});
  });

  // Make sure `read_data`'s size is the same as the number of bytes available to be read.
  std::array<uint8_t, 4> read_data;
  EXPECT_EQ(read_data.size(), kExpectedRxData.size());

  zx::result write_read_result = i2c_channel().WriteReadSync(kExpectedTxData, read_data);
  ASSERT_OK(write_read_result);

  // Verify that the correct data was written.
  i2c_server().SyncCall([](Server* server) {
    EXPECT_THAT(server->i2c_dev().tx_data(), ::testing::ElementsAreArray(kExpectedTxData));
  });

  // Verify that the correct number of bytes were read.
  EXPECT_EQ(write_read_result.value(), kExpectedRxData.size());

  // Verify that the correct bytes were read.
  EXPECT_THAT(read_data, ::testing::ElementsAreArray(kExpectedRxData));
}

// Verify that `I2cChannel::WriteReadSync()` returns the correct number of bytes read even if the
// provided read buffer is larger than what can be read.
TEST_F(I2cChannelTest, WriteReadWithLargeReadBuffer) {
  static constexpr std::array<uint8_t, 4> kExpectedRxData{0x12, 0x34, 0xab, 0xcd};
  static constexpr std::array<uint8_t, 4> kExpectedTxData{0x0f, 0x1e, 0x2d, 0x3c};

  i2c_server().SyncCall([](Server* server) {
    server->i2c_dev().set_rx_data(
        {kExpectedRxData.data(), kExpectedRxData.data() + kExpectedRxData.size()});
  });

  // Purposefully make `read_data` larger than what can be read.
  std::array<uint8_t, 5> large_read_data;
  EXPECT_GT(large_read_data.size(), kExpectedRxData.size());

  zx::result result = i2c_channel().WriteReadSync(kExpectedTxData, large_read_data);
  ASSERT_OK(result);

  // Verify that the correct number of bytes were read despite `large_read_data.size()` being larger
  // than what can be read.
  EXPECT_EQ(result.value(), kExpectedRxData.size());

  // Verify that the correct bytes were read.
  EXPECT_THAT(std::span(large_read_data.begin(), 4), ::testing::ElementsAreArray(kExpectedRxData));
}

// Verify that `I2cChannel::WriteReadSync()` returns the correct number of bytes read even if the
// provided read buffer is smaller than what can be read.
TEST_F(I2cChannelTest, WriteReadWithSmallReadBuffer) {
  static constexpr std::array<uint8_t, 4> kExpectedRxData{0x12, 0x34, 0xab, 0xcd};
  static constexpr std::array<uint8_t, 4> kExpectedTxData{0x0f, 0x1e, 0x2d, 0x3c};

  i2c_server().SyncCall([](Server* server) {
    server->i2c_dev().set_rx_data(
        {kExpectedRxData.data(), kExpectedRxData.data() + kExpectedRxData.size()});
  });

  // Purposefully make `read_data` smaller than what can be read.
  std::array<uint8_t, 3> small_read_data;
  EXPECT_LT(small_read_data.size(), kExpectedRxData.size());

  zx::result result = i2c_channel().WriteReadSync(kExpectedTxData, small_read_data);
  ASSERT_OK(result);

  // Verify that only `small_read_data.size()` bytes were read even though there is more bytes that
  // can be read.
  EXPECT_EQ(result.value(), small_read_data.size());

  // Verify that the correct bytes were read.
  EXPECT_THAT(small_read_data, ::testing::ElementsAreArray(std::span{kExpectedRxData.begin(), 3}));
}

// Verify that `I2cChannel::WriteReadySync()` does not write any data but can still read data when
// provided an empty write buffer.
TEST_F(I2cChannelTest, WriteReadEmptyWriteBuffer) {
  static constexpr std::array<uint8_t, 4> kExpectedRxData{0x12, 0x34, 0xab, 0xcd};

  i2c_server().SyncCall([](Server* server) {
    server->i2c_dev().set_rx_data(
        {kExpectedRxData.data(), kExpectedRxData.data() + kExpectedRxData.size()});
  });

  // Purposefully make the write data empty.
  static constexpr std::array<uint8_t, 0> kEmptyWriteData;
  ASSERT_TRUE(kEmptyWriteData.empty());

  std::array<uint8_t, 4> read_data;
  zx::result result = i2c_channel().WriteReadSync(kEmptyWriteData, read_data);
  ASSERT_OK(result);

  // Verify no data was written to the i2c server.
  i2c_server().SyncCall(
      [](Server* server) { EXPECT_THAT(server->i2c_dev().tx_data(), ::testing::IsEmpty()); });

  // Verify that the correct number of bytes was read.
  EXPECT_EQ(result.value(), read_data.size());

  // Verify that the correct bytes were read.
  EXPECT_THAT(read_data, kExpectedRxData);
}

class IncomingNamespace {
 public:
  explicit IncomingNamespace(async_dispatcher_t* dispatcher)
      : i2c_{dispatcher}, outgoing_(dispatcher) {}

  void Init(fidl::ServerEnd<fuchsia_io::Directory> root_server,
            std::optional<std::string_view> i2c_parent_name) {
    if (i2c_parent_name.has_value()) {
      ASSERT_OK(outgoing_.AddService<fuchsia_hardware_i2c::Service>(i2c_.GetInstanceHandler(),
                                                                    i2c_parent_name.value()));
    } else {
      ASSERT_OK(outgoing_.AddService<fuchsia_hardware_i2c::Service>(i2c_.GetInstanceHandler()));
    }

    ASSERT_OK(outgoing_.Serve(std::move(root_server)));
  }

  I2cServer<I2cDevice>& i2c() { return i2c_; }

 private:
  I2cServer<I2cDevice> i2c_;
  component::OutgoingDirectory outgoing_;
};

class I2cChannelServiceTest : public ::testing::Test {
 public:
  using Server = I2cServer<I2cDevice>;

  void Init(std::optional<std::string_view> i2c_parent_name) {
    auto [root_client, root_server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
    ASSERT_OK(incoming_namespace_loop_.StartThread("incoming-namespace"));
    incoming_namespace_.SyncCall(&IncomingNamespace::Init, std::move(root_server), i2c_parent_name);

    std::vector<fuchsia_component_runner::ComponentNamespaceEntry> entries;
    entries.push_back({{.path = std::string("/"), .directory = std::move(root_client)}});
    incoming_ = fdf::Namespace::Create(entries).value();
  }

 protected:
  void WithI2cServer(fit::callback<void(I2cServer<I2cDevice>&)> callback) {
    incoming_namespace_.SyncCall(
        [callback = std::move(callback)](auto* incoming) mutable { callback(incoming->i2c()); });
  }

  fdf::Namespace& incoming() { return incoming_; }

 private:
  async::Loop incoming_namespace_loop_{&kAsyncLoopConfigNeverAttachToThread};
  async_patterns::TestDispatcherBound<IncomingNamespace> incoming_namespace_{
      incoming_namespace_loop_.dispatcher(), std::in_place, async_patterns::PassDispatcher};
  fdf::Namespace incoming_;
};

// Verify that `I2cChannel::FromIncoming()` can create an `I2cChannel` instance that is connected to
// the default i2c instance of an incoming namespace.
TEST_F(I2cChannelServiceTest, FromDefaultInstance) {
  Init(std::nullopt);
  zx::result i2c_channel_result = I2cChannel::FromIncoming(incoming());
  ASSERT_OK(i2c_channel_result);
  I2cChannel i2c_channel = std::move(i2c_channel_result.value());
  ASSERT_TRUE(i2c_channel.is_valid());

  // Issue a simple call to make sure the connection is working.
  WithI2cServer([](auto& server) -> void { server.i2c_dev().set_rx_data({0xab}); });

  std::array<uint8_t, 1> read_data;
  zx::result read_result = i2c_channel.ReadSync(0x89, read_data);
  ASSERT_OK(read_result);
  EXPECT_EQ(read_result.value(), 1u);
  WithI2cServer([](auto& server) -> void {
    EXPECT_THAT(server.i2c_dev().tx_data(),
                ::testing::ElementsAreArray({static_cast<unsigned char>(0x89)}));
  });
  EXPECT_THAT(read_data, ::testing::ElementsAreArray({static_cast<unsigned char>(0xab)}));

  // Move the client and verify that the new one is functional.
  I2cChannel new_client = std::move(i2c_channel);

  WithI2cServer([](auto& server) -> void { server.i2c_dev().set_rx_data({0x12}); });
  read_result = new_client.ReadSync(0x34, read_data);
  ASSERT_OK(read_result);
  EXPECT_EQ(read_result.value(), 1u);
  WithI2cServer([](auto& server) -> void {
    EXPECT_THAT(server.i2c_dev().tx_data(),
                ::testing::ElementsAreArray({static_cast<unsigned char>(0x34)}));
  });
  EXPECT_THAT(read_data, ::testing::ElementsAreArray({static_cast<unsigned char>(0x12)}));
}

// Verify that `I2cChannel::FromIncoming()` can create an `I2cChannel` instance that is connected to
// a non-default i2c instance of an incoming namespace.
TEST_F(I2cChannelServiceTest, FromNonDefaultInstance) {
  static constexpr std::string_view kI2cParentName = "test-parent-name";
  Init(kI2cParentName);
  zx::result i2c_channel_result = I2cChannel::FromIncoming(incoming(), kI2cParentName);
  ASSERT_OK(i2c_channel_result);
  I2cChannel i2c_channel = std::move(i2c_channel_result.value());
  ASSERT_TRUE(i2c_channel.is_valid());

  WithI2cServer([](auto& server) -> void { server.i2c_dev().set_rx_data({0x56}); });

  std::array<uint8_t, 1> read_data;
  zx::result read_result = i2c_channel.ReadSync(0x78, read_data);
  ASSERT_OK(read_result);
  EXPECT_EQ(read_result.value(), 1u);
  WithI2cServer([](auto& server) -> void {
    EXPECT_THAT(server.i2c_dev().tx_data(),
                ::testing::ElementsAreArray({static_cast<unsigned char>(0x78)}));
  });
  EXPECT_THAT(read_data, ::testing::ElementsAreArray({static_cast<unsigned char>(0x56)}));
}

// Verify that `std::format()` formats an `i2c::I2cChannel::RetryResult` instance correctly.
TEST(RetryResultTest, Format) {
  {
    i2c::I2cChannel::RetryResult<> result{
        zx::error(ZX_ERR_NOT_FOUND),
        4,
    };
    EXPECT_EQ(std::format("{}", result), "ZX_ERR_NOT_FOUND after 4 retries");
  }

  {
    i2c::I2cChannel::RetryResult<size_t> result{
        zx::ok(5),
        0,
    };
    EXPECT_EQ(std::format("{}", result), "ZX_OK after 0 retries");
  }
}

}  // namespace i2c
