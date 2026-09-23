// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.io/cpp/wire.h>
#include <fidl/fuchsia.kernel/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/dispatcher.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fidl/cpp/wire/string_view.h>
#include <lib/fidl/cpp/wire/wire_messaging_declarations.h>
#include <lib/fit/internal/result.h>
#include <lib/sync/completion.h>
#include <lib/zx/object.h>
#include <lib/zx/result.h>
#include <lib/zx/time.h>
#include <lib/zx/vmo.h>
#include <zircon/errors.h>
#include <zircon/rights.h>
#include <zircon/status.h>
#include <zircon/syscalls/object.h>
#include <zircon/types.h>

#include <cstdint>
#include <memory>
#include <span>
#include <string_view>
#include <utility>

#include <zxtest/zxtest.h>

#include "src/storage/memfs/memfs.h"
#include "src/storage/memfs/vnode_dir.h"  // IWYU pragma: keep

namespace {

namespace fio = fuchsia_io;

// These are rights that are common to the various rights checks below.
const uint32_t kCommonExpectedRights =
    ZX_RIGHTS_BASIC | ZX_RIGHT_MAP | ZX_RIGHT_READ | ZX_RIGHT_GET_PROPERTY;

zx_rights_t get_rights(const zx::object_base& handle) {
  zx_info_handle_basic_t info;
  zx_status_t status = handle.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  return status == ZX_OK ? info.rights : ZX_RIGHT_NONE;
}

// The following sequence of events must occur to terminate cleanly:
// 1) Invoke "vfs.Shutdown", passing a closure.
// 2) Wait for the closure to be invoked, and for |completion| to be signalled. This implies
// that Shutdown no longer relies on the dispatch loop, nor will it attempt to continue
// accessing |completion|.
// 3) Shutdown the dispatch loop (happens automatically when the async::Loop goes out of scope).
//
// If the dispatch loop is terminated too before the vfs shutdown task completes, it may see
// "ZX_ERR_CANCELED" posted to the "vfs.Shutdown" closure instead.
void shutdown_vfs(std::unique_ptr<memfs::Memfs> vfs) {
  sync_completion_t completion;
  vfs->Shutdown([&completion](zx_status_t status) {
    EXPECT_OK(status);
    sync_completion_signal(&completion);
  });
  sync_completion_wait(&completion, zx::sec(5).get());
}

TEST(VmofileTests, ReadOnly) {
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  ASSERT_OK(loop.StartThread());
  async_dispatcher_t* dispatcher = loop.dispatcher();

  auto directory_endpoints = fidl::Endpoints<fio::Directory>::Create();

  zx::result result = memfs::Memfs::Create(dispatcher, "<tmp>");
  ASSERT_OK(result);
  auto& [vfs, root] = result.value();

  zx::vmo read_only_vmo;
  ASSERT_OK(zx::vmo::create(64, 0, &read_only_vmo));
  ASSERT_OK(read_only_vmo.write("hello, world!", 0, 13));
  ASSERT_OK(vfs->CreateFromVmo(root.get(), "greeting", read_only_vmo.get(), 0, 13));
  ASSERT_OK(vfs->ServeDirectory(std::move(root), std::move(directory_endpoints.server)));

  auto [file, server] = fidl::Endpoints<fio::File>::Create();
  auto open_result =
      fidl::WireCall(directory_endpoints.client)
          ->Open(fidl::StringView("greeting"), fio::wire::kPermReadable, {}, server.TakeChannel());
  ASSERT_OK(open_result.status());

  {
    const fidl::WireResult get_result =
        fidl::WireCall(file)->GetBackingMemory(fio::wire::VmoFlags::kRead);
    ASSERT_TRUE(get_result.ok(), "%s", get_result.FormatDescription().c_str());
    const auto& get_response = get_result.value();
    ASSERT_TRUE(get_response.is_ok(), "%s", zx_status_get_string(get_response.error_value()));
    const zx::vmo& vmo = get_response.value()->vmo;
    ASSERT_TRUE(vmo.is_valid());
    ASSERT_EQ(get_rights(vmo), kCommonExpectedRights);
    uint64_t size;
    ASSERT_OK(vmo.get_prop_content_size(&size));
    ASSERT_EQ(size, 13);
  }

  {
    const fidl::WireResult get_result = fidl::WireCall(file)->GetBackingMemory(
        fio::wire::VmoFlags::kRead | fio::wire::VmoFlags::kPrivateClone);
    ASSERT_TRUE(get_result.ok(), "%s", get_result.FormatDescription().c_str());
    const auto& get_response = get_result.value();
    ASSERT_TRUE(get_response.is_ok(), "%s", zx_status_get_string(get_response.error_value()));
    const zx::vmo& vmo = get_response.value()->vmo;
    ASSERT_TRUE(vmo.is_valid());
    ASSERT_EQ(get_rights(vmo), kCommonExpectedRights | ZX_RIGHT_SET_PROPERTY);
    uint64_t size;
    ASSERT_OK(vmo.get_prop_content_size(&size));
    ASSERT_EQ(size, 13);
  }

  {
    const fidl::WireResult get_result = fidl::WireCall(file)->GetBackingMemory(
        fio::wire::VmoFlags::kRead | fio::wire::VmoFlags::kExecute);
    ASSERT_TRUE(get_result.ok(), "%s", get_result.FormatDescription().c_str());
    const auto& get_response = get_result.value();
    ASSERT_TRUE(get_response.is_error());
    ASSERT_STATUS(get_response.error_value(), ZX_ERR_ACCESS_DENIED);
  }

  {
    const fidl::WireResult get_result = fidl::WireCall(file)->GetBackingMemory(
        fio::wire::VmoFlags::kRead | fio::wire::VmoFlags::kWrite);
    ASSERT_TRUE(get_result.ok(), "%s", get_result.FormatDescription().c_str());
    const auto& get_response = get_result.value();
    ASSERT_TRUE(get_response.is_error());
    ASSERT_STATUS(get_response.error_value(), ZX_ERR_ACCESS_DENIED);
  }

  {
    const fidl::WireResult result = fidl::WireCall(file)->Query();
    ASSERT_OK(result.status());
    const fidl::WireResponse response = result.value();
    const std::span data = response.protocol.get();
    const std::string_view protocol{reinterpret_cast<const char*>(data.data()), data.size_bytes()};
    ASSERT_EQ(protocol, std::string_view{fidl::DiscoverableProtocolName<fuchsia_io::File>});
  }

  {
    const fidl::WireResult seek_result =
        fidl::WireCall(file)->Seek(fio::wire::SeekOrigin::kStart, 7u);
    ASSERT_TRUE(seek_result.ok(), "%s", seek_result.status_string());
    const fit::result response = seek_result.value();
    ASSERT_TRUE(response.is_ok(), "%s", zx_status_get_string(response.error_value()));
    ASSERT_EQ(response.value()->offset_from_start, 7u);
  }

  shutdown_vfs(std::move(vfs));
}

TEST(VmofileTests, Executable) {
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  ASSERT_OK(loop.StartThread());
  async_dispatcher_t* dispatcher = loop.dispatcher();

  auto directory_endpoints = fidl::Endpoints<fio::Directory>::Create();

  zx::result result = memfs::Memfs::Create(dispatcher, "<tmp>");
  ASSERT_OK(result);
  auto& [vfs, root] = result.value();

  zx::vmo read_exec_vmo;
  ASSERT_OK(zx::vmo::create(64, 0, &read_exec_vmo));
  ASSERT_OK(read_exec_vmo.write("hello, world!", 0, 13));

  {
    zx::result client_end = component::Connect<fuchsia_kernel::VmexResource>();
    ASSERT_OK(client_end.status_value());
    fidl::WireResult result = fidl::WireCall(client_end.value())->Get();
    ASSERT_TRUE(result.ok(), "%s", result.FormatDescription().c_str());
    auto& response = result.value();

    ASSERT_OK(read_exec_vmo.replace_as_executable(response.resource, &read_exec_vmo));
    ASSERT_OK(vfs->CreateFromVmo(root.get(), "read_exec", read_exec_vmo.get(), 0, 13));
    ASSERT_OK(vfs->ServeDirectory(std::move(root), std::move(directory_endpoints.server)));
  }

  auto [file, server] = fidl::Endpoints<fio::File>::Create();
  auto open_result =
      fidl::WireCall(directory_endpoints.client)
          ->Open(fidl::StringView("read_exec"),
                 fio::wire::kPermReadable | fio::wire::kPermExecutable, {}, server.TakeChannel());
  ASSERT_OK(open_result.status());

  {
    const fidl::WireResult get_result =
        fidl::WireCall(file)->GetBackingMemory(fio::wire::VmoFlags::kRead);
    ASSERT_TRUE(get_result.ok(), "%s", get_result.FormatDescription().c_str());
    const auto& get_response = get_result.value();
    ASSERT_TRUE(get_response.is_ok(), "%s", zx_status_get_string(get_response.error_value()));
    const zx::vmo& vmo = get_response.value()->vmo;
    ASSERT_TRUE(vmo.is_valid());
    ASSERT_EQ(get_rights(vmo), kCommonExpectedRights);
    uint64_t size;
    ASSERT_OK(vmo.get_prop_content_size(&size));
    ASSERT_EQ(size, 13);
  }

  {
    // Providing a backing VMO with ZX_RIGHT_EXECUTE in CreateFromVmo above should cause
    // VmoFlags::EXECUTE to work.
    const fidl::WireResult get_result = fidl::WireCall(file)->GetBackingMemory(
        fio::wire::VmoFlags::kRead | fio::wire::VmoFlags::kExecute);
    ASSERT_TRUE(get_result.ok(), "%s", get_result.FormatDescription().c_str());
    const auto& get_response = get_result.value();
    ASSERT_TRUE(get_response.is_ok(), "%s", zx_status_get_string(get_response.error_value()));
    const zx::vmo& vmo = get_response.value()->vmo;
    ASSERT_TRUE(vmo.is_valid());
    ASSERT_EQ(get_rights(vmo), kCommonExpectedRights | ZX_RIGHT_EXECUTE);
    uint64_t size;
    ASSERT_OK(vmo.get_prop_content_size(&size));
    ASSERT_EQ(size, 13);
  }

  {
    const fidl::WireResult result = fidl::WireCall(file)->Query();
    ASSERT_OK(result.status());
    const fidl::WireResponse response = result.value();
    const std::span data = response.protocol.get();
    const std::string_view protocol{reinterpret_cast<const char*>(data.data()), data.size_bytes()};
    ASSERT_EQ(protocol, std::string_view{fidl::DiscoverableProtocolName<fuchsia_io::File>});
  }

  shutdown_vfs(std::move(vfs));
}

}  // namespace
