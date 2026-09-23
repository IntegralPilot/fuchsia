// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.io/cpp/common_types.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async-testing/test_loop.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/zx/result.h>
#include <lib/zx/time.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <utility>

#include <fbl/ref_ptr.h>
#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/storage/lib/vfs/cpp/fuchsia_vfs.h"
#include "src/storage/lib/vfs/cpp/managed_vfs.h"
#include "src/storage/lib/vfs/cpp/pseudo_dir.h"
#include "src/storage/lib/vfs/cpp/synchronous_vfs.h"
#include "src/storage/lib/vfs/cpp/vnode.h"

namespace {

using ::testing::_;

// Simple vnode implementation that provides a way to query whether the vfs pointer is set.
class TestNode : public fs::Vnode {
 public:
  // Vnode implementation:
  fuchsia_io::NodeProtocolKinds GetProtocols() const override {
    return fuchsia_io::NodeProtocolKinds::kFile;
  }

 private:
  friend fbl::internal::MakeRefCountedHelper<TestNode>;
  friend fbl::RefPtr<TestNode>;

  ~TestNode() override = default;
};

// A mock file designed to trigger specific failure paths in `Vfs::Open` and `Vfs::DeprecatedOpen`
// to verify that the vnode is correctly closed upon failure (e.g. failing rights validation by
// rejecting `kExecute` rights).
class MockFile : public fs::Vnode {
 public:
  MockFile() = default;

  fuchsia_io::NodeProtocolKinds GetProtocols() const override {
    return fuchsia_io::NodeProtocolKinds::kFile;
  }

  bool ValidateRights(fuchsia_io::Rights rights) const override {
    if (rights & fuchsia_io::Rights::kExecute) {
      return false;
    }
    return true;
  }

  zx_status_t CloseNode() override {
    closed_ = true;
    return ZX_OK;
  }

  bool closed() const { return closed_; }

 private:
  friend fbl::internal::MakeRefCountedHelper<MockFile>;
  friend fbl::RefPtr<MockFile>;

  ~MockFile() override = default;

  bool closed_ = false;
};

class MockDirectory : public fs::Vnode {
 public:
  MockDirectory(fbl::RefPtr<MockFile> child) : child_(std::move(child)) {}

  fuchsia_io::NodeProtocolKinds GetProtocols() const override {
    return fuchsia_io::NodeProtocolKinds::kDirectory;
  }

  zx_status_t Lookup(std::string_view name, fbl::RefPtr<fs::Vnode>* out) override {
    if (created_) {
      *out = child_;
      return ZX_OK;
    }
    return ZX_ERR_NOT_FOUND;
  }

  zx::result<fbl::RefPtr<fs::Vnode>> Create(std::string_view name, fs::CreationType type) override {
    if (created_) {
      return zx::error(ZX_ERR_ALREADY_EXISTS);
    }
    fbl::RefPtr<fs::Vnode> redirect;
    zx_status_t status = child_->Open(&redirect);
    if (status != ZX_OK) {
      return zx::error(status);
    }
    created_ = true;
    return zx::ok(child_);
  }

 private:
  friend fbl::internal::MakeRefCountedHelper<MockDirectory>;
  friend fbl::RefPtr<MockDirectory>;

  ~MockDirectory() override = default;

  fbl::RefPtr<MockFile> child_;
  bool created_ = false;
};

}  // namespace

// ManagedVfs always sets the dispatcher in its constructor, and trying to change it using
// Vfs::SetDispatcher should fail.
TEST(ManagedVfs, CantSetDispatcher) {
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  fs::ManagedVfs vfs(loop.dispatcher());
  ASSERT_DEATH(vfs.SetDispatcher(loop.dispatcher()), _);
}

TEST(SynchronousVfs, CanOnlySetDispatcherOnce) {
  fs::SynchronousVfs vfs;
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  vfs.SetDispatcher(loop.dispatcher());

  ASSERT_DEATH(vfs.SetDispatcher(loop.dispatcher()), _);
}

static void CheckClosesConnection(fs::FuchsiaVfs* vfs, async::TestLoop* loop) {
  zx::result a = fidl::CreateEndpoints<fuchsia_io::Directory>();
  zx::result b = fidl::CreateEndpoints<fuchsia_io::Directory>();
  ASSERT_EQ(a.status_value(), ZX_OK);
  ASSERT_EQ(b.status_value(), ZX_OK);

  auto dir_a = fbl::MakeRefCounted<fs::PseudoDir>();
  auto dir_b = fbl::MakeRefCounted<fs::PseudoDir>();
  ASSERT_EQ(vfs->ServeDirectory(dir_a, std::move(a->server)), ZX_OK);
  ASSERT_EQ(vfs->ServeDirectory(dir_b, std::move(b->server)), ZX_OK);
  bool callback_called = false;
  vfs->CloseAllConnectionsForVnode(*dir_a, [&callback_called]() { callback_called = true; });
  loop->RunUntilIdle();
  zx_signals_t signals;
  ASSERT_EQ(a->client.channel().wait_one(ZX_CHANNEL_PEER_CLOSED, zx::time::infinite(), &signals),
            ZX_OK);
  ASSERT_TRUE(signals & ZX_CHANNEL_PEER_CLOSED);
  ASSERT_EQ(ZX_ERR_TIMED_OUT,
            b->client.channel().wait_one(ZX_CHANNEL_PEER_CLOSED, zx::time(0), &signals));
  ASSERT_TRUE(callback_called);
}

TEST(ManagedVfs, CloseAllConnections) {
  async::TestLoop loop;
  fs::ManagedVfs vfs(loop.dispatcher());
  CheckClosesConnection(&vfs, &loop);
  loop.RunUntilIdle();
}

TEST(SynchronousVfs, CloseAllConnections) {
  async::TestLoop loop;
  fs::SynchronousVfs vfs(loop.dispatcher());
  CheckClosesConnection(&vfs, &loop);
  loop.RunUntilIdle();
}

TEST(ManagedVfs, CloseAllConnectionsForVnodeWithoutAnyConnections) {
  async::TestLoop loop;
  fs::ManagedVfs vfs(loop.dispatcher());
  auto dir = fbl::MakeRefCounted<fs::PseudoDir>();
  bool closed = false;
  vfs.CloseAllConnectionsForVnode(*dir, [&closed]() { closed = true; });
  loop.RunUntilIdle();
  ASSERT_TRUE(closed);
}

TEST(SynchronousVfs, CloseAllConnectionsForVnodeWithoutAnyConnections) {
  async::TestLoop loop;
  fs::SynchronousVfs vfs(loop.dispatcher());
  auto dir = fbl::MakeRefCounted<fs::PseudoDir>();
  bool closed = false;
  vfs.CloseAllConnectionsForVnode(*dir, [&closed]() { closed = true; });
  loop.RunUntilIdle();
  ASSERT_TRUE(closed);
}

TEST(VfsTest, FileClosedIfOpenFailed) {
  async::TestLoop loop;
  fs::SynchronousVfs vfs(loop.dispatcher());

  auto file = fbl::MakeRefCounted<MockFile>();
  auto dir = fbl::MakeRefCounted<MockDirectory>(file);

  fuchsia_io::Flags flags = fuchsia_io::Flags::kFlagMaybeCreate | fuchsia_io::Flags::kProtocolFile |
                            fuchsia_io::Flags::kPermExecute;

  // This call will internally:
  // 1. Traverse `dir`.
  // 2. Call `CreateOrLookup`, which calls `dir->Create()` to create and open `file`.
  // 3. Fail validation because we requested execute rights on a non-executable `MockFile`.
  auto open_result = vfs.Open(dir, "new_file", flags, nullptr, fuchsia_io::Rights::kExecute);

  ASSERT_TRUE(open_result.is_error());
  ASSERT_EQ(open_result.error_value(), ZX_ERR_ACCESS_DENIED);

  // Verify that the created file was closed because Open failed validation.
  EXPECT_TRUE(file->closed());
}

TEST(VfsTest, TruncateIgnoredOnCreation) {
  async::TestLoop loop;
  fs::SynchronousVfs vfs(loop.dispatcher());

  auto file = fbl::MakeRefCounted<MockFile>();
  auto dir = fbl::MakeRefCounted<MockDirectory>(file);

  fuchsia_io::Flags flags = fuchsia_io::Flags::kFlagMaybeCreate | fuchsia_io::Flags::kProtocolFile |
                            fuchsia_io::Flags::kFileTruncate;

  auto open_result = vfs.Open(dir, "new_file", flags, nullptr, fuchsia_io::Rights::kReadBytes);

  // We expect the open to succeed because the file is being newly created and the truncate flag is
  // ignored. If it had attempted to truncate, the open would have failed with ZX_ERR_NOT_SUPPORTED
  // because MockFile does not support truncate.
  ASSERT_TRUE(open_result.is_ok());
}

TEST(VfsTest, TruncateCalledOnExisting) {
  async::TestLoop loop;
  fs::SynchronousVfs vfs(loop.dispatcher());

  auto file = fbl::MakeRefCounted<MockFile>();
  auto dir = fbl::MakeRefCounted<MockDirectory>(file);

  fuchsia_io::Flags create_flags =
      fuchsia_io::Flags::kFlagMaybeCreate | fuchsia_io::Flags::kProtocolFile;
  auto create_result =
      vfs.Open(dir, "existing_file", create_flags, nullptr, fuchsia_io::Rights::kReadBytes);
  ASSERT_TRUE(create_result.is_ok());

  fuchsia_io::Flags truncate_flags =
      fuchsia_io::Flags::kProtocolFile | fuchsia_io::Flags::kFileTruncate;
  auto open_result =
      vfs.Open(dir, "existing_file", truncate_flags, nullptr, fuchsia_io::Rights::kReadBytes);

  // We expect the open to fail with ZX_ERR_NOT_SUPPORTED because the file already exists and the
  // VFS attempts to call Truncate on the node. Since MockFile does not support truncation, the
  // Truncate call returns ZX_ERR_NOT_SUPPORTED.
  ASSERT_TRUE(open_result.is_error());
  ASSERT_EQ(open_result.error_value(), ZX_ERR_NOT_SUPPORTED);
}

#if FUCHSIA_API_LEVEL_LESS_THAN(32) || FUCHSIA_API_LEVEL_AT_LEAST(PLATFORM)
TEST(VfsTest, DeprecatedTruncateNotCalledOnCreation) {
  async::TestLoop loop;
  fs::SynchronousVfs vfs(loop.dispatcher());

  auto file = fbl::MakeRefCounted<MockFile>();
  auto dir = fbl::MakeRefCounted<MockDirectory>(file);

  fs::DeprecatedOptions options;
  options.flags = fuchsia_io::OpenFlags::kCreate | fuchsia_io::OpenFlags::kTruncate;

  auto open_result = vfs.DeprecatedOpen(dir, "new_file", options, fuchsia_io::Rights::kReadBytes);

  // We expect the open to succeed because the file is being newly created and the truncate flag is
  // ignored. If it had attempted to truncate, the open would have failed with ZX_ERR_NOT_SUPPORTED
  // because MockFile does not support truncate.
  ASSERT_TRUE(open_result.is_ok());
}

TEST(VfsTest, DeprecatedTruncateCalledOnExisting) {
  async::TestLoop loop;
  fs::SynchronousVfs vfs(loop.dispatcher());

  auto file = fbl::MakeRefCounted<MockFile>();
  auto dir = fbl::MakeRefCounted<MockDirectory>(file);

  fs::DeprecatedOptions create_options;
  create_options.flags = fuchsia_io::OpenFlags::kCreate;
  auto create_result =
      vfs.DeprecatedOpen(dir, "existing_file", create_options, fuchsia_io::Rights::kReadBytes);
  ASSERT_TRUE(create_result.is_ok());

  fs::DeprecatedOptions truncate_options;
  truncate_options.flags = fuchsia_io::OpenFlags::kTruncate;
  auto open_result =
      vfs.DeprecatedOpen(dir, "existing_file", truncate_options, fuchsia_io::Rights::kReadBytes);

  // We expect the open to fail with ZX_ERR_NOT_SUPPORTED because the file already exists and the
  // VFS attempts to call Truncate on the node. Since MockFile does not support truncation, the
  // Truncate call returns ZX_ERR_NOT_SUPPORTED.
  ASSERT_TRUE(open_result.is_error());
  ASSERT_EQ(open_result.error(), ZX_ERR_NOT_SUPPORTED);
}
#endif
