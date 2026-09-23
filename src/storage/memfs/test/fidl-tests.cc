// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <fidl/fuchsia.fs/cpp/common_types.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fdio/cpp/caller.h>
#include <lib/fdio/directory.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fidl/cpp/wire/wire_messaging_declarations.h>
#include <lib/zx/result.h>
#include <unistd.h>
#include <zircon/errors.h>
#include <zircon/limits.h>
#include <zircon/syscalls.h>
#include <zircon/types.h>

#include <cstdint>
#include <cstring>
#include <future>
#include <span>
#include <string_view>
#include <utility>

#include <fbl/unique_fd.h>
#include <zxtest/zxtest.h>

#include "src/storage/memfs/mounted_memfs.h"

namespace {

namespace fio = fuchsia_io;

TEST(FidlTests, TestFidlBasic) {
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  ASSERT_OK(loop.StartThread());

  zx::result memfs = MountedMemfs::Create(loop.dispatcher(), "/fidltmp");
  ASSERT_OK(memfs);
  fbl::unique_fd fd(open("/fidltmp", O_DIRECTORY | O_RDONLY));
  ASSERT_GE(fd.get(), 0);

  // Create a file
  const char* filename = "file-a";
  fd.reset(openat(fd.get(), filename, O_CREAT | O_RDWR, S_IRUSR | S_IWUSR));
  ASSERT_GE(fd.get(), 0);
  const char* data = "hello";
  ssize_t datalen = strlen(data);
  ASSERT_EQ(write(fd.get(), data, datalen), datalen);
  fd.reset();

  auto [client, server] = fidl::Endpoints<fio::File>::Create();
  ASSERT_OK(fdio_open3("/fidltmp/file-a", uint64_t{fio::wire::kPermReadable},
                       server.TakeChannel().release()));

  {
    const fidl::WireResult result = fidl::WireCall(client)->Query();
    ASSERT_OK(result.status());
    const fidl::WireResponse response = result.value();
    const std::span data = response.protocol.get();
    const std::string_view protocol{reinterpret_cast<const char*>(data.data()), data.size_bytes()};
    ASSERT_EQ(protocol, std::string_view{fidl::DiscoverableProtocolName<fuchsia_io::File>});
  }
  const fidl::WireResult describe_result = fidl::WireCall(client)->Describe();
  ASSERT_OK(describe_result.status());
  ASSERT_FALSE(describe_result.value().has_observer());
  ASSERT_OK(fidl::WireCall(client)->Close());
  client.TakeChannel();

  std::promise<zx_status_t> promise;
  memfs.value()->Shutdown([&promise](zx_status_t status) { promise.set_value(status); });
  ASSERT_EQ(promise.get_future().get(), ZX_OK);

  loop.Shutdown();
}

TEST(FidlTests, TestFidlOpenReadOnly) {
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  ASSERT_OK(loop.StartThread());

  zx::result memfs = MountedMemfs::Create(loop.dispatcher(), "/fidltmp-ro");
  ASSERT_OK(memfs);
  fbl::unique_fd fd(open("/fidltmp-ro", O_DIRECTORY | O_RDONLY));
  ASSERT_GE(fd.get(), 0);

  // Create a file
  const char* filename = "file-ro";
  fd.reset(openat(fd.get(), filename, O_CREAT | O_RDWR, S_IRUSR | S_IWUSR));
  ASSERT_GE(fd.get(), 0);
  fd.reset();

  auto endpoints = fidl::Endpoints<fio::Node>::Create();
  ASSERT_OK(fdio_open3("/fidltmp-ro/file-ro", static_cast<uint64_t>(fio::wire::kPermReadable),
                       endpoints.server.TakeChannel().release()));

  auto result = fidl::WireCall(endpoints.client)->DeprecatedGetFlags();
  ASSERT_OK(result.status());
  ASSERT_OK(result->s);
  ASSERT_EQ(result->flags, fio::wire::OpenFlags::kRightReadable);
  endpoints.client.TakeChannel().reset();

  std::promise<zx_status_t> promise;
  memfs.value()->Shutdown([&promise](zx_status_t status) { promise.set_value(status); });
  ASSERT_EQ(promise.get_future().get(), ZX_OK);

  loop.Shutdown();
}

void QueryInfo(const char* path, fuchsia_io::wire::FilesystemInfo* info) {
  fbl::unique_fd fd(open(path, O_RDONLY | O_DIRECTORY));
  ASSERT_TRUE(fd);
  fdio_cpp::FdioCaller caller(std::move(fd));
  auto result = fidl::WireCall(caller.node())->QueryFilesystem();
  ASSERT_OK(result.status());
  ASSERT_OK(result->s);
  ASSERT_NOT_NULL(result->info.get());
  *info = *(result->info);
  const char* kFsName = "memfs";
  const char* name = reinterpret_cast<const char*>(info->name.data());
  ASSERT_EQ(strncmp(name, kFsName, strlen(kFsName)), 0, "Unexpected filesystem mounted");
  ASSERT_EQ(info->block_size, zx_system_get_page_size());
  ASSERT_EQ(info->max_filename_size, NAME_MAX);
  ASSERT_EQ(info->fs_type, fidl::ToUnderlying(fuchsia_fs::VfsType::kMemfs));
  ASSERT_NE(info->fs_id, 0);
  ASSERT_EQ(info->used_bytes % info->block_size, 0);
}

TEST(FidlTests, TestFidlQueryFilesystem) {
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  ASSERT_OK(loop.StartThread());

  zx::result memfs = MountedMemfs::Create(loop.dispatcher(), "/fidltmp-basic");
  ASSERT_OK(memfs);
  fbl::unique_fd fd(open("/fidltmp-basic", O_DIRECTORY | O_RDONLY));
  ASSERT_GE(fd.get(), 0);

  // Sanity checks
  fuchsia_io::wire::FilesystemInfo info;
  ASSERT_NO_FATAL_FAILURE(QueryInfo("/fidltmp-basic", &info));

  ASSERT_EQ(info.total_bytes, zx_system_get_physmem());
  ASSERT_EQ(info.used_bytes, 0);

  std::promise<zx_status_t> promise;
  memfs.value()->Shutdown([&promise](zx_status_t status) { promise.set_value(status); });
  ASSERT_EQ(promise.get_future().get(), ZX_OK);

  loop.Shutdown();
}

}  // namespace
