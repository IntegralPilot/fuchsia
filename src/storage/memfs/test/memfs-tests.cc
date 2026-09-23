// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <dirent.h>
#include <fcntl.h>
#include <fidl/fuchsia.fs/cpp/common_types.h>
#include <fidl/fuchsia.io/cpp/markers.h>
#include <fidl/fuchsia.io/cpp/wire_types.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fdio/fd.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fit/defer.h>
#include <lib/zx/result.h>
#include <lib/zx/stream.h>
#include <lib/zx/vmo.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>
#include <zircon/errors.h>
#include <zircon/rights.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/object.h>
#include <zircon/types.h>

#include <cerrno>
#include <cstdint>
#include <cstdio>
#include <future>
#include <limits>
#include <string>
#include <thread>
#include <utility>
#include <vector>

#include <fbl/ref_ptr.h>
#include <fbl/unique_fd.h>
#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"
#include "src/storage/lib/vfs/cpp/vfs_types.h"
#include "src/storage/lib/vfs/cpp/vnode.h"
#include "src/storage/memfs/memfs.h"
#include "src/storage/memfs/vnode_dir.h"

namespace memfs {
namespace {

zx_status_t GetMaxFileSize(uint64_t* size) {
  zx::vmo vmo;

  // Use the size of an unbounded VMO as the maximum vmo size.
  zx_status_t status = zx::vmo::create(0, ZX_VMO_UNBOUNDED, &vmo);
  if (status != ZX_OK) {
    return status;
  }

  return vmo.get_size(size);
}

TEST(MemfsTests, TestMemfsBasic) {
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);

  // Create a memfs filesystem, acquire a file descriptor
  zx::result result = memfs::Memfs::Create(loop.dispatcher(), "<tmp>");
  ASSERT_TRUE(result.is_ok()) << result.status_string();
  auto& [memfs, root] = result.value();
  auto shutdown = fit::defer([&memfs = memfs]() {
    std::promise<zx_status_t> promise;
    memfs->Shutdown([&promise](zx_status_t status) { promise.set_value(status); });
    ASSERT_EQ(promise.get_future().get(), ZX_OK);
  });

  auto [client, server] = fidl::Endpoints<fuchsia_io::Directory>::Create();

  ASSERT_EQ(memfs->ServeDirectory(std::move(root), std::move(server)), ZX_OK);
  ASSERT_EQ(loop.StartThread(), ZX_OK);

  fbl::unique_fd fd;
  ASSERT_EQ(fdio_fd_create(client.TakeChannel().release(), fd.reset_and_get_address()), ZX_OK);

  // Access files within the filesystem.
  DIR* d = fdopendir(fd.get());
  ASSERT_NE(d, nullptr);
  auto close_dir = fit::defer([d]() { ASSERT_EQ(closedir(d), 0) << strerror(errno); });

  // Create a file
  constexpr char filename[] = "file-a";
  {
    fbl::unique_fd fd(openat(dirfd(d), filename, O_CREAT | O_RDWR, S_IRUSR | S_IWUSR));
    ASSERT_TRUE(fd.is_valid()) << strerror(errno);
    constexpr char data[] = "hello";
    constexpr ssize_t datalen = sizeof(data);
    ASSERT_EQ(write(fd.get(), data, datalen), datalen);
    ASSERT_EQ(lseek(fd.get(), 0, SEEK_SET), 0);
    char buf[32];
    ASSERT_EQ(read(fd.get(), buf, sizeof(buf)), datalen);
    ASSERT_EQ(memcmp(buf, data, datalen), 0);
  }

  // Readdir the file
  struct dirent* de;
  ASSERT_NE((de = readdir(d)), nullptr);
  ASSERT_EQ(strcmp(de->d_name, "."), 0);
  ASSERT_NE((de = readdir(d)), nullptr);
  ASSERT_EQ(strcmp(de->d_name, filename), 0);
  ASSERT_EQ(readdir(d), nullptr);
}

TEST(MemfsTests, TestMemfsAppend) {
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);

  // Create a memfs filesystem, acquire a file descriptor
  zx::result result = memfs::Memfs::Create(loop.dispatcher(), "<tmp>");
  ASSERT_TRUE(result.is_ok()) << result.status_string();
  auto& [memfs, root] = result.value();
  auto shutdown = fit::defer([&memfs = memfs]() {
    std::promise<zx_status_t> promise;
    memfs->Shutdown([&promise](zx_status_t status) { promise.set_value(status); });
    ASSERT_EQ(promise.get_future().get(), ZX_OK);
  });

  auto [client, server] = fidl::Endpoints<fuchsia_io::Directory>::Create();

  ASSERT_EQ(memfs->ServeDirectory(std::move(root), std::move(server)), ZX_OK);
  ASSERT_EQ(loop.StartThread(), ZX_OK);

  fbl::unique_fd fd;
  ASSERT_EQ(fdio_fd_create(client.TakeChannel().release(), fd.reset_and_get_address()), ZX_OK);

  // Access files within the filesystem.
  DIR* d = fdopendir(fd.get());
  ASSERT_NE(d, nullptr);
  auto close_dir = fit::defer([d]() { ASSERT_EQ(closedir(d), 0) << strerror(errno); });

  // Create a file
  constexpr char filename[] = "file-a";
  {
    fbl::unique_fd fd(openat(dirfd(d), filename, O_CREAT | O_RDWR | O_APPEND, S_IRUSR | S_IWUSR));
    ASSERT_TRUE(fd.is_valid()) << strerror(errno);
    {
      constexpr char data[] = "hello";
      constexpr ssize_t datalen = sizeof(data) - 1;
      ASSERT_EQ(write(fd.get(), data, datalen), datalen);
    }
    ASSERT_EQ(lseek(fd.get(), 0, SEEK_SET), 0);
    {
      constexpr char data[] = ", world";
      constexpr ssize_t datalen = sizeof(data) - 1;
      ASSERT_EQ(write(fd.get(), data, datalen), datalen);
    }
    ASSERT_EQ(lseek(fd.get(), 0, SEEK_CUR), 12);
    ASSERT_EQ(lseek(fd.get(), 0, SEEK_SET), 0);
    {
      constexpr char data[] = "hello, world";
      constexpr ssize_t datalen = sizeof(data) - 1;
      char buf[32];
      ASSERT_EQ(read(fd.get(), buf, sizeof(buf)), datalen);
      ASSERT_EQ(memcmp(buf, data, datalen), 0);
    }
  }
}

TEST(MemfsTests, TestMemfsCloseDuringAccess) {
  for (int i = 0; i < 100; i++) {
    async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);

    // Create a memfs filesystem, acquire a file descriptor
    zx::result result = memfs::Memfs::Create(loop.dispatcher(), "<tmp>");
    ASSERT_TRUE(result.is_ok()) << result.status_string();
    auto& [memfs, root] = result.value();
    auto shutdown = fit::defer([&memfs = memfs]() {
      std::promise<zx_status_t> promise;
      memfs->Shutdown([&promise](zx_status_t status) { promise.set_value(status); });
      ASSERT_EQ(promise.get_future().get(), ZX_OK);
    });

    auto [client, server] = fidl::Endpoints<fuchsia_io::Directory>::Create();

    ASSERT_EQ(memfs->ServeDirectory(std::move(root), std::move(server)), ZX_OK);
    ASSERT_EQ(loop.StartThread(), ZX_OK);

    fbl::unique_fd fd;
    ASSERT_EQ(fdio_fd_create(client.TakeChannel().release(), fd.reset_and_get_address()), ZX_OK);

    // Access files within the filesystem.
    DIR* d = fdopendir(fd.get());
    ASSERT_NE(d, nullptr);
    auto close_dir = fit::defer([d]() { ASSERT_EQ(closedir(d), 0) << strerror(errno); });

    // Create the file.
    constexpr char filename[] = "foo";
    {
      fbl::unique_fd fd(openat(dirfd(d), filename, O_CREAT | O_RDWR, S_IRUSR | S_IWUSR));
      ASSERT_TRUE(fd.is_valid()) << strerror(errno);
    }

    std::thread worker([d, filename]() {
      for (size_t i = 0;; ++i) {
        fbl::unique_fd fd(openat(dirfd(d), filename, O_RDWR));
        if (!fd.is_valid()) {
          ASSERT_EQ(errno, EPIPE) << "fd=" << fd.get() << "\n"
                                  << "i=" << i << "\n"
                                  << strerror(errno);
          break;
        }
        if (close(fd.get()) != 0) {
          ASSERT_EQ(errno, EPIPE) << "fd=" << fd.get() << "\n"
                                  << "i=" << i << "\n"
                                  << strerror(errno);
          break;
        }
      }
    });

    shutdown.call();

    worker.join();

    // Now that the filesystem has terminated, we should be
    // unable to access it.
    ASSERT_LT(openat(dirfd(d), filename, O_RDWR), 0);
    ASSERT_EQ(errno, EPIPE) << "Expected connection to remote server to be closed: "
                            << strerror(errno);
  }
}

TEST(MemfsTests, TestMemfsOverflow) {
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  ASSERT_EQ(loop.StartThread(), ZX_OK);

  // Create a memfs filesystem, acquire a file descriptor
  zx::result result = memfs::Memfs::Create(loop.dispatcher(), "<tmp>");
  ASSERT_TRUE(result.is_ok()) << result.status_string();
  auto& [memfs, root] = result.value();
  auto shutdown = fit::defer([&memfs = memfs]() {
    std::promise<zx_status_t> promise;
    memfs->Shutdown([&promise](zx_status_t status) { promise.set_value(status); });
    ASSERT_EQ(promise.get_future().get(), ZX_OK);
  });

  auto [client, server] = fidl::Endpoints<fuchsia_io::Directory>::Create();

  ASSERT_EQ(memfs->ServeDirectory(std::move(root), std::move(server)), ZX_OK);
  ASSERT_EQ(loop.StartThread(), ZX_OK);

  fbl::unique_fd root_fd;
  ASSERT_EQ(fdio_fd_create(client.TakeChannel().release(), root_fd.reset_and_get_address()), ZX_OK);

  // Access files within the filesystem.
  DIR* d = fdopendir(root_fd.get());
  ASSERT_NE(d, nullptr);
  auto close_dir = fit::defer([d]() { ASSERT_EQ(closedir(d), 0) << strerror(errno); });

  // Issue writes to the file in an order that previously would have triggered
  // an overflow in the memfs write path.
  //
  // Values provided mimic the bug reported by syzkaller (https://fxbug.dev/42108793).
  uint8_t buf[4096];
  memset(buf, 'a', sizeof(buf));
  fbl::unique_fd fd(openat(dirfd(d), "file", O_CREAT | O_RDWR, S_IRUSR | S_IWUSR));
  ASSERT_TRUE(fd.is_valid()) << strerror(errno);
  ASSERT_EQ(pwrite(fd.get(), buf, 199, 0), 199);
  ASSERT_EQ(pwrite(fd.get(), buf, 226, 0xfffffffffffff801), -1);
  ASSERT_EQ(errno, EINVAL) << strerror(errno);
}

TEST(MemfsTests, TestMemfsDetachLinkedFilesystem) {
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  ASSERT_EQ(loop.StartThread(), ZX_OK);

  // Create a memfs filesystem, acquire a file descriptor
  zx::result result = memfs::Memfs::Create(loop.dispatcher(), "<tmp>");
  ASSERT_TRUE(result.is_ok()) << result.status_string();
  auto& [memfs, root] = result.value();
  auto shutdown = fit::defer([&memfs = memfs]() {
    std::promise<zx_status_t> promise;
    memfs->Shutdown([&promise](zx_status_t status) { promise.set_value(status); });
    ASSERT_EQ(promise.get_future().get(), ZX_OK);
  });

  auto [client, server] = fidl::Endpoints<fuchsia_io::Directory>::Create();

  ASSERT_EQ(memfs->ServeDirectory(std::move(root), std::move(server)), ZX_OK);
  ASSERT_EQ(loop.StartThread(), ZX_OK);

  fbl::unique_fd root_fd;
  ASSERT_EQ(fdio_fd_create(client.TakeChannel().release(), root_fd.reset_and_get_address()), ZX_OK);

  // Access files within the filesystem.
  DIR* d = fdopendir(root_fd.get());
  ASSERT_NE(d, nullptr);
  auto close_dir = fit::defer([d]() { ASSERT_EQ(closedir(d), 0) << strerror(errno); });

  // Leave a regular file.
  fbl::unique_fd fd(openat(dirfd(d), "file", O_CREAT | O_RDWR, S_IRUSR | S_IWUSR));
  ASSERT_TRUE(fd.is_valid()) << strerror(errno);

  // Leave an empty subdirectory.
  ASSERT_EQ(0, mkdirat(dirfd(d), "empty-subdirectory", 0));

  // Leave a subdirectory with children.
  ASSERT_EQ(0, mkdirat(dirfd(d), "subdirectory", 0));
  ASSERT_EQ(0, mkdirat(dirfd(d), "subdirectory/child", 0));
}

TEST(MemfsTest, DirectoryLifetime) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);

  zx::result result = memfs::Memfs::Create(loop.dispatcher(), "<tmp>");
  ASSERT_TRUE(result.is_ok()) << result.status_string();
}

TEST(MemfsTest, CreateFile) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);

  zx::result result = memfs::Memfs::Create(loop.dispatcher(), "<tmp>");
  ASSERT_TRUE(result.is_ok()) << result.status_string();
  auto& [vfs, root] = result.value();
  zx::result file = root->Create("foobar", fs::CreationType::kFile);
  ASSERT_TRUE(file.is_ok()) << file.status_string();
  auto directory = static_cast<fbl::RefPtr<fs::Vnode>>(root);

  zx::result directory_attr = directory->GetAttributes();
  ASSERT_TRUE(directory_attr.is_ok()) << directory_attr.status_string();
  zx::result file_attr = file.value()->GetAttributes();
  ASSERT_TRUE(file_attr.is_ok()) << file_attr.status_string();

  // Directory created before file.
  ASSERT_LE(directory_attr->creation_time, file_attr->creation_time);

  // Observe that the modify time of the directory is larger than the file.
  // This implies "the file is created, then the directory is updated".
  ASSERT_GE(directory_attr->modification_time, file_attr->modification_time);
}

TEST(MemfsTest, SubdirectoryUpdateTime) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);

  zx::result result = memfs::Memfs::Create(loop.dispatcher(), "<tmp>");
  ASSERT_TRUE(result.is_ok()) << result.status_string();
  auto& [vfs, root] = result.value();
  zx::result index = root->Create("index", fs::CreationType::kFile);
  ASSERT_TRUE(index.is_ok()) << index.status_string();
  zx::result subdirectory = root->Create("subdirectory", fs::CreationType::kDirectory);
  ASSERT_TRUE(subdirectory.is_ok()) << subdirectory.status_string();

  // Create a file at "subdirectory/file".
  zx::result file = subdirectory->Create("file", fs::CreationType::kFile);
  ASSERT_TRUE(file.is_ok()) << file.status_string();

  // Modify a file at "index".
  index->Truncate(10);

  zx::result subdirectory_attr = subdirectory->GetAttributes();
  ASSERT_TRUE(subdirectory_attr.is_ok()) << subdirectory_attr.status_string();
  zx::result index_attr = index->GetAttributes();
  ASSERT_TRUE(index_attr.is_ok()) << index_attr.status_string();

  // "index" was modified after "subdirectory".
  ASSERT_LE(subdirectory_attr->modification_time, index_attr->modification_time);
}

TEST(MemfsTest, SubPageContentSize) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);

  zx::result result = memfs::Memfs::Create(loop.dispatcher(), "<tmp>");
  ASSERT_TRUE(result.is_ok()) << result.status_string();
  auto& [vfs, root] = result.value();

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  // Set the content size to a non page aligned value.
  zx_off_t content_size = zx_system_get_page_size() / 2;
  EXPECT_OK(vmo.set_property(ZX_PROP_VMO_CONTENT_SIZE, &content_size, sizeof(content_size)));

  // Duplicate the handle to create the VMO file so we can use the original handle for testing.
  zx::vmo vmo_dup;
  ASSERT_OK(vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo_dup));
  // Create a VMO file sized to its content.
  ASSERT_OK(root->CreateFromVmo("vmo", vmo_dup.release(), 0, content_size));

  // Lookup the VMO and request its representation.
  fbl::RefPtr<fs::Vnode> vmo_vnode;
  ASSERT_OK(root->Lookup("vmo", &vmo_vnode));
  zx::vmo vnode_vmo;
  ASSERT_EQ(ZX_OK, vmo_vnode->GetVmo(fuchsia_io::wire::VmoFlags::kRead, &vnode_vmo));

  // We expect no cloning to have happened, this means we should have a handle to our original VMO.
  // We can verify this by comparing koids.
  zx_info_handle_basic_t original_vmo_info;
  ASSERT_OK(vmo.get_info(ZX_INFO_HANDLE_BASIC, &original_vmo_info, sizeof(original_vmo_info),
                         nullptr, nullptr));
  zx_info_handle_basic_t vnode_vmo_info;
  ASSERT_OK(vnode_vmo.get_info(ZX_INFO_HANDLE_BASIC, &vnode_vmo_info, sizeof(vnode_vmo_info),
                               nullptr, nullptr));
  EXPECT_EQ(original_vmo_info.koid, vnode_vmo_info.koid);
}

TEST(MemfsTest, LocalClone) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);

  zx::result result = memfs::Memfs::Create(loop.dispatcher(), "<tmp>");
  ASSERT_TRUE(result.is_ok()) << result.status_string();
  auto& [vfs, root] = result.value();

  zx_off_t vmo_size = zx_system_get_page_size() * static_cast<zx_off_t>(2);
  zx_off_t vmo_offset = vmo_size / 2;

  // Offset is required to be page aligned and non-zero.
  ASSERT_EQ(vmo_offset % zx_system_get_page_size(), 0ull);
  ASSERT_NE(vmo_offset, 0ull);

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(vmo_size, 0, &vmo));

  zx_info_handle_basic_t original_vmo_info;
  ASSERT_OK(vmo.get_info(ZX_INFO_HANDLE_BASIC, &original_vmo_info, sizeof(original_vmo_info),
                         nullptr, nullptr));

  // Create a file from a VMO using a non-zero offset after which we should NOT get an exact copy.
  zx::vmo vmo_dup;
  fbl::RefPtr<fs::Vnode> vmo_vnode;
  zx_info_handle_basic_t vnode_vmo_info;

  ASSERT_OK(vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo_dup));
  ASSERT_OK(root->CreateFromVmo("vmo1", vmo_dup.release(), vmo_offset, vmo_size - vmo_offset));
  ASSERT_OK(root->Lookup("vmo1", &vmo_vnode));
  zx::vmo vnode_vmo;
  ASSERT_EQ(ZX_OK, vmo_vnode->GetVmo(fuchsia_io::wire::VmoFlags::kRead, &vnode_vmo));
  ASSERT_OK(vnode_vmo.get_info(ZX_INFO_HANDLE_BASIC, &vnode_vmo_info, sizeof(vnode_vmo_info),
                               nullptr, nullptr));
  EXPECT_NE(original_vmo_info.koid, vnode_vmo_info.koid);

  // Create a file from a VMO using a smaller size, after which we should NOT get an exact copy.
  ASSERT_OK(vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo_dup));
  ASSERT_OK(root->CreateFromVmo("vmo2", vmo_dup.release(), 0, vmo_size - 1));
  ASSERT_OK(root->Lookup("vmo2", &vmo_vnode));
  ASSERT_EQ(ZX_OK, vmo_vnode->GetVmo(fuchsia_io::wire::VmoFlags::kRead, &vnode_vmo));
  ASSERT_OK(vnode_vmo.get_info(ZX_INFO_HANDLE_BASIC, &vnode_vmo_info, sizeof(vnode_vmo_info),
                               nullptr, nullptr));
  EXPECT_NE(original_vmo_info.koid, vnode_vmo_info.koid);
}

TEST(MemfsTest, TruncateZerosTail) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);

  zx::result result = memfs::Memfs::Create(loop.dispatcher(), "<tmp>");
  ASSERT_TRUE(result.is_ok()) << result.status_string();
  auto& [vfs, root] = result.value();

  zx::result file = root->Create("file", fs::CreationType::kFile);
  ASSERT_TRUE(file.is_ok()) << file.status_string();

  zx::result<zx::stream> stream = file->CreateStream(ZX_STREAM_MODE_READ | ZX_STREAM_MODE_WRITE);
  ASSERT_TRUE(stream.is_ok()) << stream.status_string();
  ASSERT_TRUE(stream->is_valid());

  std::string data = "file-contents";
  zx_iovec_t iov = {
      .buffer = data.data(),
      .capacity = data.size(),
  };
  size_t actual = 0;
  ASSERT_OK(stream->writev_at(0, 500, &iov, 1, &actual));
  ASSERT_EQ(actual, data.size());

  // Shrink the file to before the write.
  file->Truncate(300);
  // Grow the file back to after the write.
  file->Truncate(600);

  // Verify the data is gone.
  std::vector<char> file_contents(data.size(), 0);
  iov.buffer = file_contents.data();
  ASSERT_OK(stream->readv_at(0, 500, &iov, 1, &actual));
  ASSERT_EQ(actual, data.size());
  EXPECT_THAT(file_contents, testing::Each('\0'));
}

TEST(MemfsTest, WriteMaxFileSize) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);

  zx::result result = memfs::Memfs::Create(loop.dispatcher(), "<tmp>");
  ASSERT_TRUE(result.is_ok()) << result.status_string();
  auto& [vfs, root] = result.value();

  zx::result file = root->Create("file", fs::CreationType::kFile);
  ASSERT_TRUE(file.is_ok()) << file.status_string();

  zx::result<zx::stream> stream = file->CreateStream(ZX_STREAM_MODE_READ | ZX_STREAM_MODE_WRITE);
  ASSERT_TRUE(stream.is_ok()) << stream.status_string();
  ASSERT_TRUE(stream->is_valid());

  std::string data = "1";
  zx_iovec_t iov = {
      .buffer = data.data(),
      .capacity = data.size(),
  };

  uint64_t max_file_size = 0;
  ASSERT_OK(GetMaxFileSize(&max_file_size));

  size_t actual = 0;
  ASSERT_OK(stream->writev_at(0, max_file_size - 1, &iov, 1, &actual));
  ASSERT_EQ(actual, data.size());

  zx::result attributes = file->GetAttributes();
  ASSERT_TRUE(attributes.is_ok()) << attributes.status_string();
  ASSERT_EQ(attributes->content_size, max_file_size);

  // Try to write beyond the max file size.
  ASSERT_STATUS(stream->writev_at(0, max_file_size, &iov, 1, &actual), ZX_ERR_OUT_OF_RANGE);
}

TEST(MemfsTest, TruncateToMaxFileSize) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);

  zx::result result = memfs::Memfs::Create(loop.dispatcher(), "<tmp>");
  ASSERT_TRUE(result.is_ok()) << result.status_string();
  auto& [vfs, root] = result.value();

  uint64_t max_file_size = 0;
  ASSERT_OK(GetMaxFileSize(&max_file_size));

  zx::result file = root->Create("file", fs::CreationType::kFile);
  ASSERT_OK(file.status_value());
  ASSERT_OK(file->Truncate(max_file_size));
  zx::result attributes = file->GetAttributes();
  ASSERT_TRUE(attributes.is_ok()) << attributes.status_string();
  ASSERT_EQ(attributes->content_size, max_file_size);

  // Try to truncate beyond the max file size.
  ASSERT_STATUS(file->Truncate(max_file_size + 1), ZX_ERR_OUT_OF_RANGE);
}

TEST(MemfsTest, GetFilesystemInfo) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);

  zx::result result = memfs::Memfs::Create(loop.dispatcher(), "<tmp>");
  ASSERT_TRUE(result.is_ok()) << result.status_string();
  auto& [vfs, root] = result.value();

  zx::result<fs::FilesystemInfo> info_or = vfs->GetFilesystemInfo();
  ASSERT_TRUE(info_or.is_ok()) << info_or.status_string();
  const fs::FilesystemInfo& info = info_or.value();

  EXPECT_EQ(info.total_bytes, zx_system_get_physmem());
  EXPECT_EQ(info.used_bytes, 0u);
  EXPECT_EQ(info.total_nodes, UINT64_MAX);
  EXPECT_EQ(info.name, "memfs");
  EXPECT_EQ(info.block_size, zx_system_get_page_size());
  EXPECT_EQ(info.fs_type, fuchsia_fs::VfsType::kMemfs);
}

}  // namespace
}  // namespace memfs
