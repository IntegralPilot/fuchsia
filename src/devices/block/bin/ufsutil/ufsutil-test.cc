// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "ufsutil.h"

#include <fidl/fuchsia.hardware.ufs/cpp/wire_test_base.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>

#include <algorithm>
#include <cstdlib>

#include <zxtest/zxtest.h>

namespace ufsutil {

class UfsUtilTest : public zxtest::Test,
                    public fidl::testing::WireTestBase<fuchsia_hardware_ufs::Ufs> {
 public:
  UfsUtilTest() : loop_(&kAsyncLoopConfigAttachToCurrentThread) {
    ufsutil::Initialize();
    data_.resize(fuchsia_hardware_ufs::wire::kMaxDescriptorSize);
    data_[0] = static_cast<uint8_t>(fuchsia_hardware_ufs::wire::kMaxDescriptorSize);

    auto endpoints = fidl::Endpoints<fuchsia_hardware_ufs::Ufs>::Create();
    client_ = std::move(endpoints.client);
    fidl::BindServer(loop_.dispatcher(), std::move(endpoints.server), this);
    loop_.StartThread("ufs-util-test-loop");
  }

  void ReadDescriptor(fuchsia_hardware_ufs::wire::UfsReadDescriptorRequest* request,
                      ReadDescriptorCompleter::Sync& completer) override {
    completer.ReplySuccess(fidl::VectorView<uint8_t>::FromExternal(data_));
  }

  void WriteDescriptor(fuchsia_hardware_ufs::wire::UfsWriteDescriptorRequest* request,
                       WriteDescriptorCompleter::Sync& completer) override {
    std::ranges::copy(request->data, data_.begin());
    completer.ReplySuccess();
  }
  void ReadFlag(fuchsia_hardware_ufs::wire::UfsReadFlagRequest* request,
                ReadFlagCompleter::Sync& completer) override {
    completer.ReplySuccess(0);
  }
  void SetFlag(fuchsia_hardware_ufs::wire::UfsSetFlagRequest* request,
               SetFlagCompleter::Sync& completer) override {
    completer.ReplySuccess(0);
  }

  void ClearFlag(::fuchsia_hardware_ufs::wire::UfsClearFlagRequest* request,
                 ClearFlagCompleter::Sync& completer) override {
    completer.ReplySuccess(0);
  }

  void ToggleFlag(::fuchsia_hardware_ufs::wire::UfsToggleFlagRequest* request,
                  ToggleFlagCompleter::Sync& completer) override {
    completer.ReplySuccess(0);
  }

  void ReadAttribute(::fuchsia_hardware_ufs::wire::UfsReadAttributeRequest* request,
                     ReadAttributeCompleter::Sync& completer) override {
    completer.ReplySuccess(0);
  }

  void WriteAttribute(::fuchsia_hardware_ufs::wire::UfsWriteAttributeRequest* request,
                      WriteAttributeCompleter::Sync& completer) override {
    completer.ReplySuccess();
  }
  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override { FAIL(); }

  fidl::ClientEnd<fuchsia_hardware_ufs::Ufs> client_;
  async::Loop loop_;
  std::vector<uint8_t> data_;
};

using UfsClient = fidl::WireSyncClient<fuchsia_hardware_ufs::Ufs>;
TEST_F(UfsUtilTest, UnknownCommand) {
  const char* argv[] = {"ufsutil", "<device>", "unknown"};
  EXPECT_EQ(EXIT_FAILURE,
            RunUfsUtils(UfsClient(std::move(client_)), std::size(argv), const_cast<char**>(argv)));
}

TEST_F(UfsUtilTest, ReadDescriptor) {
  const char* argv[] = {"ufsutil", "<device>", "read-desc", "--type", "0"};
  EXPECT_EQ(EXIT_SUCCESS,
            RunUfsUtils(UfsClient(std::move(client_)), std::size(argv), const_cast<char**>(argv)));
}

TEST_F(UfsUtilTest, ReadConfigDescriptor) {
  const char* argv[] = {"ufsutil", "<device>", "read-desc", "--type", "1"};
  EXPECT_EQ(EXIT_SUCCESS,
            RunUfsUtils(UfsClient(std::move(client_)), std::size(argv), const_cast<char**>(argv)));
}

TEST_F(UfsUtilTest, ReadUnitDescriptor) {
  const char* argv[] = {"ufsutil", "<device>", "read-desc", "--type", "2", "--index", "0"};
  EXPECT_EQ(EXIT_SUCCESS,
            RunUfsUtils(UfsClient(std::move(client_)), std::size(argv), const_cast<char**>(argv)));
}

TEST_F(UfsUtilTest, ReadInterconnectDescriptor) {
  const char* argv[] = {"ufsutil", "<device>", "read-desc", "--type", "4", "-s", "0"};
  EXPECT_EQ(EXIT_SUCCESS,
            RunUfsUtils(UfsClient(std::move(client_)), std::size(argv), const_cast<char**>(argv)));
}

TEST_F(UfsUtilTest, ReadGeometryDescriptor) {
  const char* argv[] = {"ufsutil", "<device>", "read-desc", "--type", "7"};
  EXPECT_EQ(EXIT_SUCCESS,
            RunUfsUtils(UfsClient(std::move(client_)), std::size(argv), const_cast<char**>(argv)));
}

TEST_F(UfsUtilTest, ReadPowerDescriptor) {
  const char* argv[] = {"ufsutil", "<device>", "read-desc", "--type", "8"};
  EXPECT_EQ(EXIT_SUCCESS,
            RunUfsUtils(UfsClient(std::move(client_)), std::size(argv), const_cast<char**>(argv)));
}

TEST_F(UfsUtilTest, ReadStringDescriptor) {
  const char* argv[] = {"ufsutil", "<device>", "read-desc", "--type", "5"};
  EXPECT_EQ(EXIT_SUCCESS,
            RunUfsUtils(UfsClient(std::move(client_)), std::size(argv), const_cast<char**>(argv)));
}

TEST_F(UfsUtilTest, ReadDeviceHealthDescriptor) {
  const char* argv[] = {"ufsutil", "<device>", "read-desc", "--type", "9"};
  EXPECT_EQ(EXIT_SUCCESS,
            RunUfsUtils(UfsClient(std::move(client_)), std::size(argv), const_cast<char**>(argv)));
}

TEST_F(UfsUtilTest, ReadDescriptorUnsupportedType) {
  const char* argv[] = {"ufsutil", "<device>", "read-desc", "--type", "3"};
  EXPECT_EQ(EXIT_FAILURE,
            RunUfsUtils(UfsClient(std::move(client_)), std::size(argv), const_cast<char**>(argv)));
}

TEST_F(UfsUtilTest, ReadDescriptorInvalidArgument) {
  const char* argv[] = {"ufsutil", "<device>", "read-desc", "-t", "abc"};
  EXPECT_EQ(EXIT_FAILURE,
            RunUfsUtils(UfsClient(std::move(client_)), std::size(argv), const_cast<char**>(argv)));
}

TEST_F(UfsUtilTest, ReadDescriptorMissingRequiredArgs) {
  const char* argv[] = {"ufsutil", "<device>", "read-desc"};
  EXPECT_EQ(EXIT_FAILURE,
            RunUfsUtils(UfsClient(std::move(client_)), std::size(argv), const_cast<char**>(argv)));
}

TEST_F(UfsUtilTest, WriteDescriptor) {
  const char* argv[] = {"ufsutil", "<device>", "write-desc", "-t", "5", "-v", "string"};
  EXPECT_EQ(EXIT_SUCCESS,
            RunUfsUtils(UfsClient(std::move(client_)), std::size(argv), const_cast<char**>(argv)));
}

TEST_F(UfsUtilTest, WriteDescriptorMissingOption) {
  const char* argv[] = {"ufsutil", "<device>", "write-desc", "-t", "5"};
  EXPECT_EQ(EXIT_FAILURE,
            RunUfsUtils(UfsClient(std::move(client_)), std::size(argv), const_cast<char**>(argv)));
}

TEST_F(UfsUtilTest, WriteDescriptorUnsupportedType) {
  const char* argv[] = {"ufsutil", "<device>", "write-desc", "--type", "3"};
  EXPECT_EQ(EXIT_FAILURE,
            RunUfsUtils(UfsClient(std::move(client_)), std::size(argv), const_cast<char**>(argv)));
}

TEST_F(UfsUtilTest, WriteDescriptorMissingRequiredArgs) {
  const char* argv[] = {"ufsutil", "<device>", "write-desc", "-t"};
  EXPECT_EQ(EXIT_FAILURE,
            RunUfsUtils(UfsClient(std::move(client_)), std::size(argv), const_cast<char**>(argv)));
}

TEST_F(UfsUtilTest, WriteDescriptorNotWritable) {
  const char* argv[] = {"ufsutil", "<device>", "write-desc", "-t", "0"};
  EXPECT_EQ(EXIT_FAILURE,
            RunUfsUtils(UfsClient(std::move(client_)), std::size(argv), const_cast<char**>(argv)));
}

TEST_F(UfsUtilTest, SetFlag) {
  const char* argv[] = {"ufsutil", "<device>", "set-flag", "-t", "1"};
  EXPECT_EQ(EXIT_SUCCESS,
            RunUfsUtils(UfsClient(std::move(client_)), std::size(argv), const_cast<char**>(argv)));
}

TEST_F(UfsUtilTest, SetFlagReadOnly) {
  const char* argv[] = {"ufsutil", "<device>", "set-flag", "-t", "9"};
  EXPECT_EQ(EXIT_FAILURE,
            RunUfsUtils(UfsClient(std::move(client_)), std::size(argv), const_cast<char**>(argv)));
}

TEST_F(UfsUtilTest, SetFlagUnsupportedType) {
  const char* argv[] = {"ufsutil", "<device>", "set-flag", "-t", "99"};
  EXPECT_EQ(EXIT_FAILURE,
            RunUfsUtils(UfsClient(std::move(client_)), std::size(argv), const_cast<char**>(argv)));
}

TEST_F(UfsUtilTest, ReadFlag) {
  const char* argv[] = {"ufsutil", "<device>", "read-flag", "-t", "1"};
  EXPECT_EQ(EXIT_SUCCESS,
            RunUfsUtils(UfsClient(std::move(client_)), std::size(argv), const_cast<char**>(argv)));
}

TEST_F(UfsUtilTest, ReadFlagWriteOnly) {
  const char* argv[] = {"ufsutil", "<device>", "read-flag", "-t", "6"};
  EXPECT_EQ(EXIT_FAILURE,
            RunUfsUtils(UfsClient(std::move(client_)), std::size(argv), const_cast<char**>(argv)));
}

TEST_F(UfsUtilTest, ToggleFlag) {
  const char* argv[] = {"ufsutil", "<device>", "toggle-flag", "-t", "1"};
  EXPECT_EQ(EXIT_SUCCESS,
            RunUfsUtils(UfsClient(std::move(client_)), std::size(argv), const_cast<char**>(argv)));
}

TEST_F(UfsUtilTest, ClearFlag) {
  const char* argv[] = {"ufsutil", "<device>", "clear-flag", "-t", "1"};
  EXPECT_EQ(EXIT_SUCCESS,
            RunUfsUtils(UfsClient(std::move(client_)), std::size(argv), const_cast<char**>(argv)));
}

TEST_F(UfsUtilTest, ReadAttribute) {
  const char* argv[] = {"ufsutil", "<device>", "read-attr", "-t", "3"};
  EXPECT_EQ(EXIT_SUCCESS,
            RunUfsUtils(UfsClient(std::move(client_)), std::size(argv), const_cast<char**>(argv)));
}

TEST_F(UfsUtilTest, ReadAttributeWriteOnly) {
  const char* argv[] = {"ufsutil", "<device>", "read-attr", "-t", "0xf"};
  EXPECT_EQ(EXIT_FAILURE,
            RunUfsUtils(UfsClient(std::move(client_)), std::size(argv), const_cast<char**>(argv)));
}

TEST_F(UfsUtilTest, ReadAttributeUnsupportedType) {
  const char* argv[] = {"ufsutil", "<device>", "read-attr", "-t", "99"};
  EXPECT_EQ(EXIT_FAILURE,
            RunUfsUtils(UfsClient(std::move(client_)), std::size(argv), const_cast<char**>(argv)));
}

TEST_F(UfsUtilTest, WriteAttribute) {
  const char* argv[] = {"ufsutil", "<device>", "write-attr", "-t", "0", "-v", "0x01"};
  EXPECT_EQ(EXIT_SUCCESS,
            RunUfsUtils(UfsClient(std::move(client_)), std::size(argv), const_cast<char**>(argv)));
}

TEST_F(UfsUtilTest, WriteAttributeReadOnly) {
  const char* argv[] = {"ufsutil", "<device>", "write-attr", "-t", "5", "-v", "0x01"};
  EXPECT_EQ(EXIT_FAILURE,
            RunUfsUtils(UfsClient(std::move(client_)), std::size(argv), const_cast<char**>(argv)));
}

TEST_F(UfsUtilTest, WriteAttributeMissingValue) {
  const char* argv[] = {"ufsutil", "<device>", "write-attr", "-t", "5"};
  EXPECT_EQ(EXIT_FAILURE,
            RunUfsUtils(UfsClient(std::move(client_)), std::size(argv), const_cast<char**>(argv)));
}

}  // namespace ufsutil
