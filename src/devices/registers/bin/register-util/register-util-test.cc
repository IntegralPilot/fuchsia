// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "register-util.h"

#include <fidl/fuchsia.hardware.registers/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>

#include <zxtest/zxtest.h>

class PhyServer : public fidl::WireServer<fuchsia_hardware_registers::Device> {
 public:
  PhyServer() : loop_(&kAsyncLoopConfigNeverAttachToThread) {
    auto channels = fidl::CreateEndpoints<fuchsia_hardware_registers::Device>();
    channel_ = std::move(channels->client);
    fidl::BindServer(loop_.dispatcher(), std::move(channels->server), this);
    loop_.StartThread();
  }
  void ReadRegister8(ReadRegister8RequestView request,
                     ReadRegister8Completer::Sync& completer) override {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }
  void ReadRegister16(ReadRegister16RequestView request,
                      ReadRegister16Completer::Sync& completer) override {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }
  void ReadRegister32(ReadRegister32RequestView request,
                      ReadRegister32Completer::Sync& completer) override {
    address_ = request->offset;
    completer.ReplySuccess(value_);
  }
  void ReadRegister64(ReadRegister64RequestView request,
                      ReadRegister64Completer::Sync& completer) override {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }
  void WriteRegister8(WriteRegister8RequestView request,
                      WriteRegister8Completer::Sync& completer) override {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }
  void WriteRegister16(WriteRegister16RequestView request,
                       WriteRegister16Completer::Sync& completer) override {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }
  void WriteRegister32(WriteRegister32RequestView request,
                       WriteRegister32Completer::Sync& completer) override {
    address_ = request->offset;
    value_ = request->value;
    completer.ReplySuccess();
  }
  void WriteRegister64(WriteRegister64RequestView request,
                       WriteRegister64Completer::Sync& completer) override {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }
  fidl::ClientEnd<fuchsia_hardware_registers::Device> TakeChannel() { return std::move(channel_); }
  uint64_t address() const { return address_; }

  uint64_t value() const { return value_; }

 private:
  uint64_t address_ = 0;
  uint32_t value_ = 0;
  async::Loop loop_;
  fidl::ClientEnd<fuchsia_hardware_registers::Device> channel_;
};

TEST(RegisterUtil, RegisterReadTest) {
  const char* args[] = {"", "/bin/register-util", "50"};
  PhyServer server;
  ASSERT_EQ(run(3, args, server.TakeChannel()), 0);
  ASSERT_EQ(server.address(), 0x50);
}

TEST(RegisterUtil, RegisterWriteTest) {
  const char* args[] = {"", "/bin/register-util", "50", "90"};
  PhyServer server;
  ASSERT_EQ(run(4, args, server.TakeChannel()), 0);
  ASSERT_EQ(server.address(), 0x50);
  ASSERT_EQ(server.value(), 0x90);
}
