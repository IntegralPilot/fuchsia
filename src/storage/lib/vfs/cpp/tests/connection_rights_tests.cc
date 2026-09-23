// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.io/cpp/common_types.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fidl/cpp/wire/wire_messaging_declarations.h>
#include <lib/sync/completion.h>
#include <lib/zx/result.h>
#include <lib/zx/time.h>
#include <lib/zx/vmo.h>
#include <zircon/errors.h>
#include <zircon/status.h>
#include <zircon/types.h>

#include <memory>
#include <utility>

#include <fbl/ref_ptr.h>
#include <gtest/gtest.h>

#include "src/storage/lib/vfs/cpp/managed_vfs.h"
#include "src/storage/lib/vfs/cpp/vfs_types.h"

namespace {

namespace fio = fuchsia_io;

class TestVNode : public fs::Vnode {
 public:
  fuchsia_io::NodeProtocolKinds GetProtocols() const final {
    return fuchsia_io::NodeProtocolKinds::kFile;
  }
  zx_status_t GetVmo(fuchsia_io::wire::VmoFlags flags, zx::vmo* out_vmo) override {
    zx::vmo vmo;
    zx_status_t status = zx::vmo::create(4096, 0u, &vmo);
    EXPECT_EQ(status, ZX_OK);
    if (status != ZX_OK)
      return status;
    *out_vmo = std::move(vmo);
    return ZX_OK;
  }
};

TEST(ConnectionRightsTest, GetBackingMemory) {
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  ASSERT_EQ(loop.StartThread(), ZX_OK);

  std::unique_ptr<fs::ManagedVfs> vfs = std::make_unique<fs::ManagedVfs>(loop.dispatcher());

  struct TestCase {
    fio::Flags serve_flags;
    fio::VmoFlags get_backing_memory_flags;
    zx_status_t result;  // What we expect FileGetBuffer to return.
  };

  TestCase test_data[] = {
      // If the connection has all rights, then everything should work.
      {
          .serve_flags =
              fio::Flags::kPermReadBytes | fio::Flags::kPermWriteBytes | fio::Flags::kPermExecute,
          .get_backing_memory_flags = fio::wire::VmoFlags::kRead,
          .result = ZX_OK,
      },
      {
          .serve_flags =
              fio::Flags::kPermReadBytes | fio::Flags::kPermWriteBytes | fio::Flags::kPermExecute,
          .get_backing_memory_flags = fio::wire::VmoFlags::kRead | fio::wire::VmoFlags::kWrite,
          .result = ZX_OK,
      },
      {
          .serve_flags =
              fio::Flags::kPermReadBytes | fio::Flags::kPermWriteBytes | fio::Flags::kPermExecute,
          .get_backing_memory_flags = fio::wire::VmoFlags::kRead | fio::wire::VmoFlags::kExecute,
          .result = ZX_OK,
      },
      // If the connection is missing the EXECUTABLE right, then requests with
      // fio::wire::VmoFlags::kExecute should fail.
      {
          .serve_flags = fio::Flags::kPermReadBytes | fio::Flags::kPermWriteBytes,
          .get_backing_memory_flags = fio::wire::VmoFlags::kRead | fio::wire::VmoFlags::kExecute,
          .result = ZX_ERR_ACCESS_DENIED,
      },
      // If the connection is missing the WRITABLE right, then requests with
      // fio::wire::VmoFlags::kWrite should fail.
      {
          .serve_flags = fio::Flags::kPermReadBytes | fio::Flags::kPermExecute,
          .get_backing_memory_flags = fio::wire::VmoFlags::kRead | fio::wire::VmoFlags::kWrite,
          .result = ZX_ERR_ACCESS_DENIED,
      },
  };

  {
    auto vnode = fbl::MakeRefCounted<TestVNode>();
    for (const auto& test_case : test_data) {
      // Set up a vfs connection with the testcase's connection flags
      auto file = fidl::Endpoints<fio::File>::Create();
      auto serve_result = vfs->Serve(vnode, file.server.TakeChannel(), test_case.serve_flags);
      ASSERT_EQ(serve_result, ZX_OK) << zx_status_get_string(serve_result);
      // Call FileGetBuffer on the channel with the testcase's request flags. Check that we get the
      // expected result.
      const fidl::WireResult result =
          fidl::WireCall(file.client)->GetBackingMemory(test_case.get_backing_memory_flags);
      ASSERT_TRUE(result.ok()) << result.FormatDescription();
      const auto& response = result.value();

      // Verify that the result matches the value in our test table.
      if (test_case.result == ZX_OK) {
        EXPECT_TRUE(response.is_ok()) << zx_status_get_string(response.error_value());
      } else {
        EXPECT_TRUE(response.is_error());
        EXPECT_EQ(response.error_value(), test_case.result);
      }
    }
  }

  // Tear down the VFS. On completion, it will no longer rely on the async loop. Then, tear down the
  // async loop.
  sync_completion_t completion;
  vfs->Shutdown([&completion](zx_status_t status) {
    EXPECT_EQ(status, ZX_OK);
    sync_completion_signal(&completion);
  });
  sync_completion_wait(&completion, zx::time::infinite().get());
  loop.Shutdown();
}

}  // namespace
