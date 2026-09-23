// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.io/cpp/fidl.h>
#include <fidl/fuchsia.io/cpp/wire_test_base.h>
#include <lib/fdio/directory.h>
#include <lib/fidl/cpp/wire/connect_service.h>
#include <lib/zx/channel.h>

#include <string>

#include <fbl/unique_fd.h>

#include "src/storage/fs_test/fs_test_fixture.h"
#include "src/storage/fs_test/misc.h"

namespace fs_test {
namespace {

namespace fio = fuchsia_io;

using OpenTest = FilesystemTest;

fidl::ClientEnd<fio::Directory> CreateDirectory(fio::Flags dir_flags, const std::string& path) {
  EXPECT_EQ(mkdir(path.c_str(), 0755), 0);

  auto endpoints = fidl::Endpoints<fio::Directory>::Create();
  EXPECT_EQ(
      fdio_open3(path.c_str(), static_cast<uint64_t>(dir_flags | fio::Flags::kProtocolDirectory),
                 endpoints.server.TakeChannel().release()),
      ZX_OK);

  return std::move(endpoints.client);
}

zx_status_t OpenFileWithMaybeCreate(const fidl::ClientEnd<fio::Directory>& dir,
                                    const std::string& path) {
  fio::Flags flags = fio::Flags::kProtocolFile | fio::Flags::kFlagMaybeCreate |
                     fio::Flags::kFlagSendRepresentation;
  auto file_endpoints = fidl::Endpoints<fio::Node>::Create();
  auto open_result = fidl::WireCall(dir)->Open(fidl::StringView::FromExternal(path), flags, {},
                                               file_endpoints.server.TakeChannel());
  EXPECT_EQ(open_result.status(), ZX_OK);
  fidl::WireSyncClient child{std::move(file_endpoints.client)};

  class EventHandler : public fidl::testing::WireSyncEventHandlerTestBase<fio::Node> {
   public:
    EventHandler() = default;
    zx_status_t status() const { return status_; }

    void OnRepresentation(fidl::WireEvent<fio::Node::OnRepresentation>* representation) override {
      EXPECT_EQ(representation->Which(), fio::wire::Representation::Tag::kFile);
      status_ = ZX_OK;
    }

    void NotImplemented_(const std::string& name) override { FAIL() << "Unexpected " << name; }

   private:
    zx_status_t status_ = ZX_OK;
  };

  EventHandler event_handler;
  auto status = child.HandleOneEvent(event_handler);
  if (!status.ok()) {
    EXPECT_NE(status.reason(), fidl::Reason::kUnexpectedMessage);
    return status.status();
  }
  return event_handler.status();
}

TEST_P(OpenTest, OpenFileWithCreateCreatesInReadWriteDir) {
  constexpr fio::Flags kFlags = fio::kPermReadable | fio::kPermWritable;
  auto parent = CreateDirectory(kFlags, GetPath("a"));
  EXPECT_EQ(OpenFileWithMaybeCreate(parent, "b"), ZX_OK);
}

TEST_P(OpenTest, OpenFileWithCreateFailsInReadOnlyDir) {
  constexpr fio::Flags kFlags = fio::kPermReadable;
  auto parent = CreateDirectory(kFlags, GetPath("a"));
  EXPECT_EQ(OpenFileWithMaybeCreate(parent, "b"), ZX_ERR_ACCESS_DENIED);
}

TEST_P(OpenTest, OpenFileWithCreateCreatesInReadWriteDirWithPermInheritWrite) {
  constexpr fio::Flags kParentFlags = fio::kPermReadable | fio::kPermWritable;
  auto parent = CreateDirectory(kParentFlags, GetPath("a"));
  // kPermInheritWrite expand the rights of the directory connection to include write rights if the
  // parent connection has them.
  constexpr fio::Flags kReopenFlags = fio::kPermReadable | fio::Flags::kPermInheritWrite;
  auto [client, server] = fidl::Endpoints<fio::Directory>::Create();
  auto result = fidl::WireCall(parent)->Open(".", kReopenFlags, {}, server.TakeChannel());
  ASSERT_EQ(result.status(), ZX_OK);
  // Creating a file should succeed on `client` as it should have write permissions.
  EXPECT_EQ(OpenFileWithMaybeCreate(client, "b"), ZX_OK);
}

TEST_P(OpenTest, OpenFileWithCreateFailsInReadOnlyDirWithPermInheritWrite) {
  constexpr fio::Flags kParentFlags = fio::kPermReadable;
  auto parent = CreateDirectory(kParentFlags, GetPath("a"));
  // kPermInheritWrite expand the rights of the directory connection to include write rights if the
  // parent connection has them. As the parent directory only has read permissions, the resulting
  // connection will not have write permissions.
  constexpr fio::Flags kReopenFlags = fio::kPermReadable | fio::Flags::kPermInheritWrite;
  auto [client, server] = fidl::Endpoints<fio::Directory>::Create();
  auto reopen_result = fidl::WireCall(parent)->Open(".", kReopenFlags, {}, server.TakeChannel());
  ASSERT_EQ(reopen_result.status(), ZX_OK);
  // Creating a file should fail on `client` as it should not have write permissions.
  EXPECT_EQ(OpenFileWithMaybeCreate(client, "b"), ZX_ERR_ACCESS_DENIED);
}

INSTANTIATE_TEST_SUITE_P(/*no prefix*/, OpenTest, testing::ValuesIn(AllTestFilesystems()),
                         testing::PrintToStringParamName());

}  // namespace
}  // namespace fs_test
