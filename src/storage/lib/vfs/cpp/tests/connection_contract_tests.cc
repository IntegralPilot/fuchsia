// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.io/cpp/markers.h>
#include <fidl/fuchsia.io/cpp/natural_types.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/zx/channel.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>
#include <zircon/errors.h>

#include <memory>
#include <utility>

#include <fbl/intrusive_double_list.h>
#include <fbl/ref_ptr.h>
#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/storage/lib/vfs/cpp/connection/connection.h"
#include "src/storage/lib/vfs/cpp/fuchsia_vfs.h"
#include "src/storage/lib/vfs/cpp/pseudo_dir.h"
#include "src/storage/lib/vfs/cpp/vnode.h"

#if __has_feature(address_sanitizer) || __has_feature(leak_sanitizer) || \
    __has_feature(hwaddress_sanitizer)
#include <sanitizer/lsan_interface.h>
#endif

namespace {

using testing::_;

// Base class used to define fake Vfs objects to test |Connection::Bind|.
class NoOpVfs : public fs::FuchsiaVfs {
 public:
  using FuchsiaVfs::FuchsiaVfs;
  ~NoOpVfs() { connections_.clear(); }

 protected:
  fbl::DoublyLinkedList<fs::internal::Connection*> connections_;

 private:
  void Shutdown(ShutdownCallback handler) override {
    FAIL() << "Should never be reached in this test";
  }
  void CloseAllConnectionsForVnode(const fs::Vnode& node,
                                   CloseAllConnectionsForVnodeCallback callback) final {
    FAIL() << "Should never be reached in this test";
  }
};

// A Vfs that first places connections into a linked list before
// starting message dispatch.
class NoOpVfsGood : public NoOpVfs {
 public:
  using NoOpVfs::NoOpVfs;

 private:
  zx::result<> RegisterConnection(std::unique_ptr<fs::internal::Connection> connection,
                                  zx::channel& server_end) final {
    connections_.push_back(connection.release());
    connections_.back().Bind(std::move(server_end),
                             [](fs::internal::Connection* connection) { delete connection; });
    return zx::ok();
  }
};

// A Vfs that first starts message dispatch on a connection before placing it into a linked list.
// This behavior is racy (https://fxbug.dev/42122489) so we test that it triggers a failed
// precondition check.
class NoOpVfsBad : public NoOpVfs {
 public:
  using NoOpVfs::NoOpVfs;

 private:
  zx::result<> RegisterConnection(std::unique_ptr<fs::internal::Connection> connection,
                                  zx::channel& server_end) final {
    connection->Bind(std::move(server_end),
                     [](fs::internal::Connection* connection) { delete connection; });
    connections_.push_back(connection.release());
    return zx::ok();
  }
};

template <typename VfsType>
void RunTest(async::Loop* loop, VfsType&& vfs) {
  auto root = fbl::MakeRefCounted<fs::PseudoDir>();
  auto endpoints = fidl::CreateEndpoints<fuchsia_io::Directory>();
  ASSERT_EQ(endpoints.status_value(), ZX_OK);

  ASSERT_EQ(vfs.ServeDirectory(root, std::move(endpoints->server), fuchsia_io::kRStarDir), ZX_OK);
  loop->RunUntilIdle();
}

TEST(ConnectionContractTest, BindRequiresVfsManagingConnectionPositive) {
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  RunTest(&loop, NoOpVfsGood(loop.dispatcher()));
}

TEST(ConnectionDeathTest, BindRequiresVfsManagingConnectionNegative) {
  // Bind requires registering the connection in a list first.
  if (ZX_DEBUG_ASSERT_IMPLEMENTED) {
    async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
    ASSERT_DEATH(
        {
#if __has_feature(address_sanitizer) || __has_feature(leak_sanitizer) || \
    __has_feature(hwaddress_sanitizer)
          // Disable LSAN, this thread is expected to leak by way of a crash.
          __lsan::ScopedDisabler _;
#endif
          RunTest(&loop, NoOpVfsBad(loop.dispatcher()));
        },
        _);
  }
}

}  // namespace
