// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "sdio.h"

#include <fidl/fuchsia.hardware.sdio/cpp/wire_test_base.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fdf/arena.h>

#include <optional>
#include <vector>

#include <zxtest/zxtest.h>

namespace sdio {

namespace {

class SdioTest : public zxtest::Test,
                 public fidl::testing::WireTestBase<fuchsia_hardware_sdio::Device> {
 public:
  SdioTest() : loop_(&kAsyncLoopConfigAttachToCurrentThread), arena_(fdf::Arena('S')) {
    auto endpoints = fidl::Endpoints<Device>::Create();
    client_ = std::move(endpoints.client);
    fidl::BindServer(loop_.dispatcher(), std::move(endpoints.server), this);
    loop_.StartThread("sdio-test-loop");
  }

  void GetDevHwInfo(GetDevHwInfoCompleter::Sync& completer) override {
    fuchsia_hardware_sdio::wire::SdioHwInfo fidl_hw_info = {};
    completer.ReplySuccess(fidl_hw_info);
  }

  void EnableFn(EnableFnCompleter::Sync& completer) override {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }

  void DisableFn(DisableFnCompleter::Sync& completer) override {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }

  void EnableFnIntr(EnableFnIntrCompleter::Sync& completer) override {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }

  void DisableFnIntr(DisableFnIntrCompleter::Sync& completer) override {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }

  void UpdateBlockSize(UpdateBlockSizeRequestView request,
                       UpdateBlockSizeCompleter::Sync& completer) override {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }

  void GetBlockSize(GetBlockSizeCompleter::Sync& completer) override {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }

  void DoRwTxn(DoRwTxnRequestView request, DoRwTxnCompleter::Sync& completer) override {
    fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion> buffers(
        arena_, request->txn.buffers.size());
    for (size_t i = 0; i < request->txn.buffers.size(); i++) {
      buffers[i] = std::move(request->txn.buffers[i]);
    }
    txns_.emplace_back(wire::SdioRwTxn{.addr = request->txn.addr,
                                       .incr = request->txn.incr,
                                       .write = request->txn.write,
                                       .buffers = std::move(buffers)});
    completer.ReplySuccess();
  }

  void ReadByte(ReadByteRequestView request, ReadByteCompleter::Sync& completer) override {
    ASSERT_TRUE(byte_.has_value());
    ASSERT_FALSE(address_.has_value());
    address_ = request->address;
    completer.ReplySuccess(byte_.value());
    byte_.reset();
  }

  void WriteByte(WriteByteRequestView request, WriteByteCompleter::Sync& completer) override {
    ASSERT_FALSE(byte_.has_value());
    ASSERT_FALSE(address_.has_value());
    byte_ = request->byte;
    address_ = request->address;
    completer.ReplySuccess(request->byte);
  }

  void GetInBandIntr(GetInBandIntrCompleter::Sync& completer) override {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }

  void IoAbort(IoAbortCompleter::Sync& completer) override {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }

  void IntrPending(IntrPendingCompleter::Sync& completer) override {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }

  void DoVendorControlRwByte(DoVendorControlRwByteRequestView request,
                             DoVendorControlRwByteCompleter::Sync& completer) override {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }
  void AckInBandIntr(AckInBandIntrCompleter::Sync& completer) override {}

  void RegisterVmo(RegisterVmoRequestView request, RegisterVmoCompleter::Sync& completer) override {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }
  void UnregisterVmo(UnregisterVmoRequestView request,
                     UnregisterVmoCompleter::Sync& completer) override {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }

  void RequestCardReset(RequestCardResetCompleter::Sync& completer) override {
    reset_ = true;
    completer.ReplySuccess();
  }
  void PerformTuning(PerformTuningCompleter::Sync& completer) override {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }

  void NotImplemented_(const std::string& name, ::fidl::CompleterBase& completer) override {
    FAIL();
  }

 protected:
  void set_byte(uint8_t byte) { byte_ = byte; }
  std::optional<uint8_t> get_byte() const { return byte_; }
  std::optional<uint32_t> get_address() const { return address_; }
  const std::vector<wire::SdioRwTxn>& get_txns() { return txns_; }
  bool reset() const { return reset_; }

  static void ExpectTxnsEqual(const wire::SdioRwTxn& lhs, const wire::SdioRwTxn& rhs) {
    EXPECT_EQ(lhs.addr, rhs.addr);
    EXPECT_EQ(lhs.incr, rhs.incr);
    EXPECT_EQ(lhs.write, rhs.write);
    EXPECT_EQ(lhs.buffers.size(), rhs.buffers.size());
    EXPECT_EQ(lhs.buffers.size(), 1);
    EXPECT_EQ(lhs.buffers.begin()->buffer.Which(), rhs.buffers.begin()->buffer.Which());
    EXPECT_EQ(lhs.buffers.begin()->offset, rhs.buffers.begin()->offset);
    EXPECT_EQ(lhs.buffers.begin()->size, rhs.buffers.begin()->size);
  }

  async::Loop loop_;
  fidl::ClientEnd<Device> client_;
  fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion kExpectedBuffer = {
      .buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmo({}),
      .offset = 0,
      .size = 256,
  };

 private:
  std::optional<uint8_t> byte_;
  std::optional<uint32_t> address_;
  std::vector<wire::SdioRwTxn> txns_;
  fdf::Arena arena_;
  bool reset_ = false;
};

TEST_F(SdioTest, NoArguments) {
  EXPECT_NE(0, sdio::RunSdioTool(SdioClient(std::move(client_)), 0, nullptr));
}

TEST_F(SdioTest, UnknownCommand) {
  const char* argv[] = {"bad"};
  EXPECT_NE(0, sdio::RunSdioTool(SdioClient(std::move(client_)), 1, argv));
}

TEST_F(SdioTest, Info) {
  const char* argv[] = {"info"};
  EXPECT_EQ(0, sdio::RunSdioTool(SdioClient(std::move(client_)), 1, argv));
}

TEST_F(SdioTest, ReadByte) {
  set_byte(0xab);
  const char* argv[] = {"read-byte", "0x01234"};
  EXPECT_EQ(0, sdio::RunSdioTool(SdioClient(std::move(client_)), 2, argv));
  EXPECT_EQ(get_address(), 0x01234);
}

TEST_F(SdioTest, ReadByteBadAddress) {
  const char* argv[] = {"read-byte", "0x123zz"};
  EXPECT_NE(0, sdio::RunSdioTool(SdioClient(std::move(client_)), 2, argv));
}

TEST_F(SdioTest, WriteByte) {
  const char* argv[] = {"write-byte", "5000", "0xab"};
  EXPECT_EQ(0, sdio::RunSdioTool(SdioClient(std::move(client_)), 3, argv));
  EXPECT_EQ(get_address(), 5000);
  EXPECT_EQ(get_byte(), 0xab);
}

TEST_F(SdioTest, WriteByteBadAddress) {
  const char* argv[] = {"write-byte", "-10", "0xab"};
  EXPECT_NE(0, sdio::RunSdioTool(SdioClient(std::move(client_)), 3, argv));
}

TEST_F(SdioTest, WriteByteBadByte) {
  const char* argv[] = {"write-byte", "5000", "0x100"};
  EXPECT_NE(0, sdio::RunSdioTool(SdioClient(std::move(client_)), 3, argv));
}

TEST_F(SdioTest, WriteByteNotEnoughArguments) {
  const char* argv[] = {"write-byte", "5000"};
  EXPECT_NE(0, sdio::RunSdioTool(SdioClient(std::move(client_)), 2, argv));
}

TEST_F(SdioTest, ReadStress) {
  const char* argv[] = {"read-stress", "0x10000", "256", "20"};
  EXPECT_EQ(0, sdio::RunSdioTool(SdioClient(std::move(client_)), 4, argv));
  EXPECT_EQ(get_txns().size(), 20);

  const wire::SdioRwTxn kExpectedTxn = {
      .addr = 0x10000,
      .incr = true,
      .write = false,
      .buffers = fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion>::FromExternal(
          &kExpectedBuffer, 1),
  };

  for (const wire::SdioRwTxn& txn : get_txns()) {
    ASSERT_NO_FATAL_FAILURE(ExpectTxnsEqual(txn, kExpectedTxn));
  }
}

TEST_F(SdioTest, ReadStressFifo) {
  const char* argv[] = {"read-stress", "0x10000", "256", "20", "--fifo"};
  EXPECT_EQ(0, sdio::RunSdioTool(SdioClient(std::move(client_)), 5, argv));
  EXPECT_EQ(get_txns().size(), 20);

  const wire::SdioRwTxn kExpectedTxn = {
      .addr = 0x10000,
      .incr = false,
      .write = false,
      .buffers = fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion>::FromExternal(
          &kExpectedBuffer, 1),
  };

  for (const wire::SdioRwTxn& txn : get_txns()) {
    ASSERT_NO_FATAL_FAILURE(ExpectTxnsEqual(txn, kExpectedTxn));
  }
}

TEST_F(SdioTest, ReadStressBadAddress) {
  const char* argv[] = {"read-stress", "0x20000", "256", "20", "--fifo"};
  EXPECT_NE(0, sdio::RunSdioTool(SdioClient(std::move(client_)), 5, argv));
}

TEST_F(SdioTest, ReadStressNotEnoughArguments) {
  const char* argv[] = {"read-stress", "0x10000", "256"};
  EXPECT_NE(0, sdio::RunSdioTool(SdioClient(std::move(client_)), 3, argv));
}

TEST_F(SdioTest, GetTxnStats) {
  EXPECT_STREQ(GetTxnStats(zx::sec(2), 100).c_str(), "2.000 s (50.000 B/s)");
  EXPECT_STREQ(GetTxnStats(zx::msec(2), 100).c_str(), "2.000 ms (50.000 kB/s)");
  EXPECT_STREQ(GetTxnStats(zx::usec(2), 100).c_str(), "2.000 us (50.000 MB/s)");
  EXPECT_STREQ(GetTxnStats(zx::nsec(2), 100).c_str(), "2 ns (50.000 GB/s)");
  EXPECT_STREQ(GetTxnStats(zx::usec(-2), 100).c_str(), "-2000 ns (-50000000.000 B/s)");
  EXPECT_STREQ(GetTxnStats(zx::usec(2), 0).c_str(), "2.000 us (0.000 B/s)");
  EXPECT_STREQ(GetTxnStats(zx::nsec(0), 100).c_str(), "0 ns");
}

TEST_F(SdioTest, Read) {
  const char* argv[] = {"read", "0x10000", "256"};
  EXPECT_EQ(0, sdio::RunSdioTool(SdioClient(std::move(client_)), 3, argv));
  ASSERT_EQ(get_txns().size(), 1);

  const wire::SdioRwTxn kExpectedTxn = {
      .addr = 0x10000,
      .incr = true,
      .write = false,
      .buffers = fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion>::FromExternal(
          &kExpectedBuffer, 1),
  };

  ASSERT_NO_FATAL_FAILURE(ExpectTxnsEqual(get_txns()[0], kExpectedTxn));
}

TEST_F(SdioTest, ReadFifo) {
  const char* argv[] = {"read", "0x10000", "256", "--fifo"};
  EXPECT_EQ(0, sdio::RunSdioTool(SdioClient(std::move(client_)), 4, argv));
  ASSERT_EQ(get_txns().size(), 1);

  const wire::SdioRwTxn kExpectedTxn = {
      .addr = 0x10000,
      .incr = false,
      .write = false,
      .buffers = fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion>::FromExternal(
          &kExpectedBuffer, 1),
  };

  ASSERT_NO_FATAL_FAILURE(ExpectTxnsEqual(get_txns()[0], kExpectedTxn));
}

TEST_F(SdioTest, Reset) {
  EXPECT_FALSE(reset());
  const char* argv[] = {"reset"};
  EXPECT_EQ(0, sdio::RunSdioTool(SdioClient(std::move(client_)), 1, argv));
  EXPECT_TRUE(reset());
}

}  // namespace

}  // namespace sdio
