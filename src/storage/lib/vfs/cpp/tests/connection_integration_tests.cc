// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This file includes basic VFS file/directory connection tests. For comprehensive behavioral tests,
// see the fuchsia.io Conformance Test Suite in //src/storage/conformance.

#include <fidl/fuchsia.io/cpp/common_types.h>
#include <fidl/fuchsia.io/cpp/fidl.h>
#include <fidl/fuchsia.io/cpp/wire_test_base.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fdio/directory.h>
#include <lib/fidl/cpp/channel.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fidl/cpp/wire/status.h>
#include <lib/fidl/cpp/wire/string_view.h>
#include <lib/fidl/cpp/wire/wire_messaging.h>
#include <lib/fidl/cpp/wire_natural_conversions.h>
#include <lib/zx/result.h>
#include <lib/zx/time.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/status.h>
#include <zircon/types.h>

#include <cstdint>
#include <mutex>
#include <optional>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

#include <fbl/ref_ptr.h>
#include <fbl/string.h>
#include <gtest/gtest.h>

#include "src/storage/lib/vfs/cpp/pseudo_dir.h"
#include "src/storage/lib/vfs/cpp/pseudo_file.h"
#include "src/storage/lib/vfs/cpp/synchronous_vfs.h"
#include "src/storage/lib/vfs/cpp/vfs_types.h"

namespace {

namespace fio = fuchsia_io;

zx_status_t DummyReader(fbl::String* output) { return ZX_OK; }

zx_status_t DummyWriter(std::string_view input) { return ZX_OK; }

// Example vnode that supports protocol negotiation. Here the vnode may be opened as a file or a
// directory.
class FileOrDirectory : public fs::Vnode {
 public:
  FileOrDirectory() = default;

  fuchsia_io::NodeProtocolKinds GetProtocols() const final {
    return fuchsia_io::NodeProtocolKinds::kFile | fuchsia_io::NodeProtocolKinds::kDirectory;
  }

  fs::VnodeAttributesQuery SupportedMutableAttributes() const final {
    return fs::VnodeAttributesQuery::kModificationTime;
  }

  zx::result<fs::VnodeAttributes> GetAttributes() const final {
    return zx::ok(fs::VnodeAttributes{
        .id = 1234,
        .modification_time = modification_time_,
    });
  }

  bool ValidateRights(fio::Rights rights) const final {
    return (rights & fio::Rights::kExecute) == fio::Rights();
  }

  zx::result<> UpdateAttributes(const fs::VnodeAttributesUpdate& attributes) final {
    // Attributes not reported by |SupportedMutableAttributes()| should never be set in
    // |attributes|.
    ZX_ASSERT(!attributes.creation_time);
    modification_time_ = *attributes.modification_time;
    return zx::ok();
  }

 private:
  uint64_t modification_time_;
};

// Helper method to monitor the OnRepresentation event. Used by the tests below to decode the
// fuchsia.io/Node.OnRepresentation event, or to check for the correct epitaph on errors.
zx::result<fio::Representation> GetOnRepresentation(fidl::UnownedClientEnd<fio::Node> channel) {
  struct EventHandler final : public fidl::testing::WireSyncEventHandlerTestBase<fio::Node> {
   public:
    void OnRepresentation(fidl::WireEvent<fio::Node::OnRepresentation>* event) override {
      ZX_ASSERT(event);
      representation = fidl::ToNatural(std::move(*event));
    }

    void NotImplemented_(const std::string& name) override {
      ADD_FAILURE() << "unexpected " << name;
    }

    std::optional<fio::Representation> representation = std::nullopt;
  };

  EventHandler event_handler;
  const fidl::Status result = event_handler.HandleOneEvent(channel);
  if (!result.ok()) {
    return zx::error(result.status());
  }
  ZX_ASSERT(event_handler.representation);
  return zx::ok(std::move(*event_handler.representation));
}

class VfsTestSetup : public testing::Test {
 public:
  // Setup file structure with one directory and one file. Note: On creation directories and files
  // have no flags and rights.
  VfsTestSetup() : loop_(&kAsyncLoopConfigNoAttachToCurrentThread) {
    vfs_.SetDispatcher(loop_.dispatcher());
    root_ = fbl::MakeRefCounted<fs::PseudoDir>();
    dir_ = fbl::MakeRefCounted<fs::PseudoDir>();
    file_ = fbl::MakeRefCounted<fs::BufferedPseudoFile>(&DummyReader, &DummyWriter);
    file_or_dir_ = fbl::MakeRefCounted<FileOrDirectory>();
    root_->AddEntry("dir", dir_);
    root_->AddEntry("file", file_);
    root_->AddEntry("file_or_dir", file_or_dir_);
  }

  zx_status_t ConnectClient(fidl::ServerEnd<fio::Directory> server_end) {
    // Serve root directory with maximum rights
    return vfs_.ServeDirectory(root_, std::move(server_end));
  }

  zx_status_t ConnectClient(fidl::ServerEnd<fio::Directory> server_end, fio::Rights rights) {
    return vfs_.ServeDirectory(root_, std::move(server_end), rights);
  }

  void SetReadonly() { vfs_.SetReadonly(true); }

 protected:
  void SetUp() override { loop_.StartThread(); }

  void TearDown() override { loop_.RunUntilIdle(); }

 private:
  async::Loop loop_;
  fs::SynchronousVfs vfs_;
  fbl::RefPtr<fs::PseudoDir> root_;
  fbl::RefPtr<fs::PseudoDir> dir_;
  fbl::RefPtr<fs::Vnode> file_;
  fbl::RefPtr<FileOrDirectory> file_or_dir_;
};

using ConnectionTest = VfsTestSetup;

TEST_F(ConnectionTest, GetSetFlagsOnFile) {
  // Create connection to vfs
  auto root = fidl::Endpoints<fio::Directory>::Create();
  ASSERT_EQ(ConnectClient(std::move(root.server)), ZX_OK);

  // Open a new file connection
  zx::result fc = fidl::CreateEndpoints<fio::File>();
  ASSERT_EQ(fc.status_value(), ZX_OK);
  ASSERT_EQ(fdio_open3_at(root.client.channel().get(), "file", uint64_t{fio::kPermReadable},
                          fc->server.TakeChannel().release()),
            ZX_OK);

  // Use GetFlags to get current flags and rights
  constexpr fio::Flags kExpectedFlags =
      fio::Flags::kPermReadBytes | fio::Flags::kPermGetAttributes | fio::Flags::kProtocolFile;
  auto flags = fidl::WireCall(fc->client)->GetFlags();
  ASSERT_TRUE(flags.ok() && flags->is_ok()) << flags;
  ASSERT_EQ((*flags)->flags, kExpectedFlags);
  {
    // Make modifications to flags with SetFlags
    auto result = fidl::WireCall(fc->client)->SetFlags(fio::Flags::kFileAppend);
    ASSERT_TRUE(result.ok() && result->is_ok()) << result;
  }
  {
    // Check that the new flag is saved
    auto new_flags = fidl::WireCall(fc->client)->GetFlags();
    ASSERT_TRUE(new_flags.ok() && new_flags->is_ok()) << new_flags;
    ASSERT_EQ((*new_flags)->flags, kExpectedFlags | fio::Flags::kFileAppend);
  }
}

TEST_F(ConnectionTest, GetSetFlagsOnDirectory) {
  // Create connection to vfs
  auto root = fidl::Endpoints<fio::Directory>::Create();
  ASSERT_EQ(ConnectClient(std::move(root.server)), ZX_OK);

  // Connect to Directory
  zx::result dc = fidl::CreateEndpoints<fio::Directory>();
  ASSERT_EQ(dc.status_value(), ZX_OK);
  ASSERT_EQ(fdio_open3_at(root.client.channel().get(), "dir",
                          static_cast<uint64_t>(fio::kPermReadable | fio::kPermWritable),
                          dc->server.TakeChannel().release()),
            ZX_OK);

  // Use GetFlags to get current flags and rights
  constexpr fio::Flags kExpectedFlags =
      fio::kPermReadable | fio::kPermWritable | fio::Flags::kProtocolDirectory;
  auto flags = fidl::WireCall(dc->client)->GetFlags();
  ASSERT_TRUE(flags.ok() && flags->is_ok()) << flags;
  ASSERT_EQ((*flags)->flags, kExpectedFlags);

  // Directories do not support setting flags.
  auto result = fidl::WireCall(dc->client)->SetFlags(fio::Flags::kFileAppend);
  ASSERT_TRUE(result.ok() && result->is_error()) << result;
  ASSERT_EQ(result->error_value(), ZX_ERR_NOT_SUPPORTED) << result;
}

TEST_F(ConnectionTest, InheritPermissionsDirectoryRightExpansion) {
  // Create connection to VFS with all rights.
  auto root = fidl::Endpoints<fio::Directory>::Create();
  ASSERT_EQ(ConnectClient(std::move(root.server)), ZX_OK);

  // Combinations of permission inherit flags to be tested.
  constexpr fio::Flags kFlagCombinations[]{
      fio::Flags::kPermInheritWrite, fio::Flags::kPermInheritExecute,
      fio::Flags::kPermInheritWrite | fio::Flags::kPermInheritExecute};

  for (const fio::Flags flags : kFlagCombinations) {
    // Connect to directory specifying the flag combination we want to test.
    zx::result dc = fidl::CreateEndpoints<fio::Directory>();
    ASSERT_EQ(dc.status_value(), ZX_OK);
    ASSERT_EQ(fdio_open3_at(root.client.channel().get(), "dir",
                            static_cast<uint64_t>(fio::kPermReadable | flags),
                            dc->server.TakeChannel().release()),
              ZX_OK);

    // Ensure flags match those which we expect.
    auto dir_flags = fidl::WireCall(dc->client)->GetFlags();
    ASSERT_TRUE(dir_flags.ok() && dir_flags->is_ok()) << dir_flags;
    fio::Flags expected_dir_flags = fio::kPermReadable | fio::Flags::kProtocolDirectory;
    // Each permission inherit flag should be expanded to its respective right(s).
    if (flags & fio::Flags::kPermInheritWrite)
      expected_dir_flags |= fio::kPermWritable;
    if (flags & fio::Flags::kPermInheritExecute)
      expected_dir_flags |= fio::kPermExecutable;
    ASSERT_EQ((*dir_flags)->flags, expected_dir_flags);

    // Repeat test, but for file, which should not have any expanded rights.
    auto fc = fidl::Endpoints<fio::File>::Create();
    ASSERT_EQ(fdio_open3_at(root.client.channel().get(), "file",
                            static_cast<uint64_t>(fio::kPermReadable | flags),
                            fc.server.TakeChannel().release()),
              ZX_OK);
    auto file_flags = fidl::WireCall(fc.client)->GetFlags();
    ASSERT_TRUE(file_flags.ok() && file_flags->is_ok()) << file_flags;
    // We should not have any flags that are not in kPermReadable | Flags::kProtocolFile.
    constexpr fio::Flags kAllowedFileFlags = fio::kPermReadable | fio::Flags::kProtocolFile;
    ASSERT_EQ((*file_flags)->flags & ~kAllowedFileFlags, fio::Flags{});
  }
}

// Test that the GetAttributes and UpdateAttributes methods work as expected.
TEST_F(ConnectionTest, GetUpdateAttributes) {
  // Create connection to vfs
  auto root = fidl::Endpoints<fio::Directory>::Create();
  ASSERT_EQ(ConnectClient(std::move(root.server)), ZX_OK);

  // Connect to File
  zx::result fc = fidl::CreateEndpoints<fio::File>();
  ASSERT_EQ(fc.status_value(), ZX_OK);
  ASSERT_EQ(fdio_open3_at(root.client.channel().get(), "file_or_dir",
                          static_cast<uint64_t>(fio::kPermReadable | fio::kPermWritable),
                          fc->server.TakeChannel().release()),
            ZX_OK);
  auto client = fidl::SyncClient(std::move(fc->client));
  // Our test Vnode only reports a hard-coded ID in addition to protocols/abilities.
  fio::ImmutableNodeAttributes expected_immutable_attrs = fio::ImmutableNodeAttributes();
  expected_immutable_attrs.id() = 1234;
  expected_immutable_attrs.abilities() = FileOrDirectory().GetAbilities();
  expected_immutable_attrs.protocols() = FileOrDirectory().GetProtocols();
  // Our test Vnode only supports modification time, and should default-initialize it to zero.
  fio::MutableNodeAttributes expected_mutable_attrs = fio::MutableNodeAttributes();
  expected_mutable_attrs.modification_time() = 0;
  {
    auto attrs = client->GetAttributes(fio::NodeAttributesQuery::kMask);
    ASSERT_TRUE(attrs.is_ok());
    EXPECT_EQ(attrs->immutable_attributes(), expected_immutable_attrs);
    EXPECT_EQ(attrs->mutable_attributes(), expected_mutable_attrs);
  }

  // Ensure we can't set creation time.
  {
    fio::MutableNodeAttributes update = fio::MutableNodeAttributes();
    update.creation_time() = 0;
    auto result = client->UpdateAttributes(update);
    ASSERT_TRUE(result.is_error());
    EXPECT_EQ(result.error_value().domain_error(), ZX_ERR_NOT_SUPPORTED);
  }

  // Update modification time.
  expected_mutable_attrs.modification_time() = 1234;
  {
    auto result = client->UpdateAttributes(expected_mutable_attrs);
    ASSERT_TRUE(result.is_ok());
  }

  // Check modification time was updated and other attributes remain unchanged.
  {
    auto attrs = client->GetAttributes(fio::NodeAttributesQuery::kMask);
    ASSERT_TRUE(attrs.is_ok());
    EXPECT_EQ(attrs->immutable_attributes(), expected_immutable_attrs);
    EXPECT_EQ(attrs->mutable_attributes(), expected_mutable_attrs);
  }
}

TEST_F(ConnectionTest, FileSeekDirectory) {
  // Create connection to vfs
  auto root = fidl::Endpoints<fio::Directory>::Create();
  ASSERT_EQ(ConnectClient(std::move(root.server)), ZX_OK);

  // Interacting with a Directory connection using File protocol methods should fail.
  {
    zx::result dc = fidl::CreateEndpoints<fio::Directory>();
    ASSERT_EQ(dc.status_value(), ZX_OK);
    ASSERT_EQ(fdio_open3_at(root.client.channel().get(), "dir",
                            static_cast<uint64_t>(fio::kPermReadable | fio::kPermWritable),
                            dc->server.TakeChannel().release()),
              ZX_OK);

    // Borrowing directory channel as file channel.
    auto dir_get_result =
        fidl::WireCall(fidl::UnownedClientEnd<fio::File>(dc->client.borrow().handle()))
            ->Seek(fio::wire::SeekOrigin::kStart, 0);
    EXPECT_NE(dir_get_result.status(), ZX_OK);
  }
}

TEST_F(ConnectionTest, NegotiateProtocol) {
  // Create connection to vfs
  auto root = fidl::Endpoints<fio::Directory>::Create();
  ASSERT_EQ(ConnectClient(std::move(root.server)), ZX_OK);

  {
    // Connect to polymorphic node as a directory.
    zx::result dc = fidl::CreateEndpoints<fio::Node>();
    ASSERT_EQ(dc.status_value(), ZX_OK);
    ASSERT_EQ(fidl::WireCall(root.client)
                  ->Open(fidl::StringView("file_or_dir"),
                         fio::Flags::kProtocolDirectory | fio::Flags::kFlagSendRepresentation, {},
                         dc->server.TakeChannel())
                  .status(),
              ZX_OK);
    zx::result<fio::Representation> dir_info = GetOnRepresentation(dc->client);
    ASSERT_EQ(dir_info.status_value(), ZX_OK);
    ASSERT_EQ(dir_info->Which(), fio::Representation::Tag::kDirectory);
  }

  {
    // Connect to polymorphic node as a file.
    zx::result fc = fidl::CreateEndpoints<fio::Node>();
    ASSERT_EQ(fc.status_value(), ZX_OK);
    ASSERT_EQ(fidl::WireCall(root.client)
                  ->Open(fidl::StringView("file_or_dir"),
                         fio::Flags::kProtocolFile | fio::Flags::kFlagSendRepresentation, {},
                         fc->server.TakeChannel())
                  .status(),
              ZX_OK);
    zx::result<fio::Representation> file_info = GetOnRepresentation(fc->client);
    ASSERT_EQ(file_info.status_value(), ZX_OK);
    ASSERT_EQ(file_info->Which(), fio::Representation::Tag::kFile);
  }
}

TEST_F(ConnectionTest, ValidateRights) {
  // Create connection to vfs
  auto root = fidl::Endpoints<fio::Directory>::Create();
  ASSERT_EQ(ConnectClient(std::move(root.server)), ZX_OK);
  // The test Vnode should disallow execute rights.
  {
    zx::result fc = fidl::CreateEndpoints<fio::Node>();
    ASSERT_EQ(fc.status_value(), ZX_OK);
    ASSERT_EQ(fidl::WireCall(root.client)
                  ->Open(fidl::StringView("file_or_dir"),
                         fio::Flags::kFlagSendRepresentation | fio::Flags::kProtocolFile |
                             fio::Flags::kPermExecute,
                         {}, fc->server.TakeChannel())
                  .status(),
              ZX_OK);
    zx::result<fio::Representation> file_info = GetOnRepresentation(fc->client);
    ASSERT_EQ(file_info.status_value(), ZX_ERR_ACCESS_DENIED);
  }
}

TEST_F(ConnectionTest, ValidateRightsReadonly) {
  // Set the filesystem as read-only before creating a root connection.
  SetReadonly();
  auto root = fidl::Endpoints<fio::Directory>::Create();
  ASSERT_EQ(ConnectClient(std::move(root.server)), ZX_OK);

  {
    // If the filesystem is read only, we shouldn't be able to open files as writable.
    zx::result fc = fidl::CreateEndpoints<fio::Node>();
    ASSERT_EQ(fc.status_value(), ZX_OK);
    ASSERT_EQ(fidl::WireCall(root.client)
                  ->Open(fidl::StringView("file_or_dir"),
                         fio::Flags::kFlagSendRepresentation | fio::Flags::kProtocolFile |
                             fio::kPermWritable,
                         {}, fc->server.TakeChannel())
                  .status(),
              ZX_OK);
    zx::result<fio::Representation> file_info = GetOnRepresentation(fc->client);
    ASSERT_TRUE(file_info.is_error());
    ASSERT_EQ(file_info.error_value(), ZX_ERR_ACCESS_DENIED);
  }
  {
    // If the filesystem is read only, we shouldn't be granted mutable rights for directories.
    zx::result fc = fidl::CreateEndpoints<fio::Node>();
    ASSERT_EQ(fc.status_value(), ZX_OK);
    ASSERT_EQ(fidl::WireCall(root.client)
                  ->Open(fidl::StringView("file_or_dir"),
                         fio::Flags::kFlagSendRepresentation | fio::Flags::kProtocolDirectory |
                             fio::kPermReadable | fio::Flags::kPermInheritWrite,
                         {}, fc->server.TakeChannel())
                  .status(),
              ZX_OK);
    zx::result<fio::Representation> dir_info = GetOnRepresentation(fc->client);
    ASSERT_EQ(dir_info.status_value(), ZX_OK);
    ASSERT_EQ(dir_info->Which(), fio::Representation::Tag::kDirectory);
    auto flags = fidl::WireCall(fc->client)->GetFlags();
    constexpr fio::Flags kExpectedFlags = fio::kPermReadable | fio::Flags::kProtocolDirectory;
    ASSERT_TRUE(flags.ok() && flags->is_ok()) << flags;
    ASSERT_EQ((*flags)->flags, kExpectedFlags);
  }
}

TEST_F(ConnectionTest, TraverseRights) {
  // Create connection to vfs with only GET_ATTRIBUTES right (no TRAVERSE).
  auto root = fidl::Endpoints<fio::Directory>::Create();
  ASSERT_EQ(ConnectClient(std::move(root.server), fio::Rights::kGetAttributes), ZX_OK);

  // Attempt to open a child node. This should fail with ACCESS_DENIED because we don't have
  // TRAVERSE.
  {
    zx::result fc = fidl::CreateEndpoints<fio::Node>();
    ASSERT_EQ(fc.status_value(), ZX_OK);
    ASSERT_EQ(fidl::WireCall(root.client)
                  ->Open(fidl::StringView("file"),
                         fio::Flags::kFlagSendRepresentation | fio::Flags::kProtocolFile, {},
                         fc->server.TakeChannel())
                  .status(),
              ZX_OK);
    zx::result<fio::Representation> file_info = GetOnRepresentation(fc->client);
    ASSERT_TRUE(file_info.is_error());
    ASSERT_EQ(file_info.error_value(), ZX_ERR_ACCESS_DENIED);
  }

  // Attempt to open "." (dot). This should succeed because TRAVERSE is not required for dot.
  {
    zx::result dc = fidl::CreateEndpoints<fio::Node>();
    ASSERT_EQ(dc.status_value(), ZX_OK);
    ASSERT_EQ(fidl::WireCall(root.client)
                  ->Open(fidl::StringView("."),
                         fio::Flags::kFlagSendRepresentation | fio::Flags::kProtocolDirectory, {},
                         dc->server.TakeChannel())
                  .status(),
              ZX_OK);
    zx::result<fio::Representation> dir_info = GetOnRepresentation(dc->client);
    ASSERT_EQ(dir_info.status_value(), ZX_OK);
    ASSERT_EQ(dir_info->Which(), fio::Representation::Tag::kDirectory);
  }

  // Attempt to open a child node with DeprecatedOpen without TRAVERSE. Should fail with
  // ACCESS_DENIED.
  {
    zx::result fc = fidl::CreateEndpoints<fio::Node>();
    ASSERT_EQ(fc.status_value(), ZX_OK);
    ASSERT_EQ(fidl::WireCall(root.client)
                  ->DeprecatedOpen(fio::OpenFlags::kRightReadable, {}, fidl::StringView("file"),
                                   std::move(fc->server))
                  .status(),
              ZX_OK);
    zx_signals_t observed = ZX_SIGNAL_NONE;
    ASSERT_EQ(
        fc->client.channel().wait_one(ZX_CHANNEL_PEER_CLOSED, zx::time::infinite(), &observed),
        ZX_OK);
  }

  // Attempt to open "." with DeprecatedOpen without TRAVERSE. Should succeed.
  {
    zx::result dc = fidl::CreateEndpoints<fio::Node>();
    ASSERT_EQ(dc.status_value(), ZX_OK);
    ASSERT_EQ(fidl::WireCall(root.client)
                  ->DeprecatedOpen(fio::OpenFlags::kRightReadable, {}, fidl::StringView("."),
                                   std::move(dc->server))
                  .status(),
              ZX_OK);
  }
}

// A vnode which maintains a counter of number of |Open| calls that have not been balanced out with
// a |Close|.
class CountOutstandingOpenVnode : public fs::Vnode {
 public:
  CountOutstandingOpenVnode() = default;

  fuchsia_io::NodeProtocolKinds GetProtocols() const final {
    return fuchsia_io::NodeProtocolKinds::kFile;
  }

  uint64_t GetOpenCount() const {
    std::lock_guard lock(mutex_);
    return open_count();
  }
};

class ConnectionClosingTest : public testing::Test {
 public:
  // Setup file structure with one directory and one file. Note: On creation directories and files
  // have no flags and rights.
  ConnectionClosingTest() : loop_(&kAsyncLoopConfigNoAttachToCurrentThread) {
    vfs_.SetDispatcher(loop_.dispatcher());
    root_ = fbl::MakeRefCounted<fs::PseudoDir>();
    count_outstanding_open_vnode_ = fbl::MakeRefCounted<CountOutstandingOpenVnode>();
    root_->AddEntry("count_outstanding_open_vnode", count_outstanding_open_vnode_);
  }

  zx_status_t ConnectClient(fidl::ServerEnd<fuchsia_io::Directory> server_end) {
    // Serve root directory with maximum rights
    return vfs_.ServeDirectory(root_, std::move(server_end));
  }

 protected:
  fbl::RefPtr<CountOutstandingOpenVnode> count_outstanding_open_vnode() const {
    return count_outstanding_open_vnode_;
  }

  async::Loop& loop() { return loop_; }

 private:
  async::Loop loop_;
  fs::SynchronousVfs vfs_;
  fbl::RefPtr<fs::PseudoDir> root_;
  fbl::RefPtr<CountOutstandingOpenVnode> count_outstanding_open_vnode_;
};

TEST_F(ConnectionClosingTest, ClosingChannelImpliesClosingNode) {
  // Create connection to vfs.
  auto [root_client, root_server] = fidl::Endpoints<fio::Directory>::Create();
  ASSERT_EQ(ConnectClient(std::move(root_server)), ZX_OK);

  constexpr unsigned kNumActiveClients = 20;

  ASSERT_EQ(count_outstanding_open_vnode()->GetOpenCount(), 0u);

  // Create a number of active connections to "count_outstanding_open_vnode".
  std::vector<fidl::ClientEnd<fio::Node>> clients;
  for (unsigned i = 0; i < kNumActiveClients; i++) {
    auto [client, server] = fidl::Endpoints<fio::Node>::Create();
    ASSERT_EQ(fidl::WireCall(root_client)
                  ->Open(fidl::StringView("count_outstanding_open_vnode"), fio::wire::kPermReadable,
                         {}, server.TakeChannel())
                  .status(),
              ZX_OK);
    clients.push_back(std::move(client));
  }

  ASSERT_EQ(loop().RunUntilIdle(), ZX_OK);
  ASSERT_EQ(count_outstanding_open_vnode()->GetOpenCount(), kNumActiveClients);

  // Drop all the clients, leading to |Close| being invoked on "count_outstanding_open_vnode"
  // eventually.
  clients.clear();

  ASSERT_EQ(loop().RunUntilIdle(), ZX_OK);
  ASSERT_EQ(count_outstanding_open_vnode()->GetOpenCount(), 0u);
}

TEST_F(ConnectionClosingTest, ClosingNodeLeadsToClosingServerEndChannel) {
  // Create connection to vfs.
  auto root = fidl::Endpoints<fio::Directory>::Create();
  ASSERT_EQ(ConnectClient(std::move(root.server)), ZX_OK);

  zx_signals_t observed = ZX_SIGNAL_NONE;
  ASSERT_EQ(ZX_ERR_TIMED_OUT, root.client.channel().wait_one(ZX_CHANNEL_PEER_CLOSED,
                                                             zx::time::infinite_past(), &observed));
  ASSERT_FALSE(observed & ZX_CHANNEL_PEER_CLOSED);

  ASSERT_EQ(loop().StartThread(), ZX_OK);
  auto result = fidl::WireCall(root.client)->Close();
  ASSERT_EQ(result.status(), ZX_OK);
  ASSERT_TRUE(result->is_ok()) << zx_status_get_string(result->error_value());

  observed = ZX_SIGNAL_NONE;
  ASSERT_EQ(root.client.channel().wait_one(ZX_CHANNEL_PEER_CLOSED, zx::time::infinite(), &observed),
            ZX_OK);
  ASSERT_TRUE(observed & ZX_CHANNEL_PEER_CLOSED);

  loop().Shutdown();
}

}  // namespace
