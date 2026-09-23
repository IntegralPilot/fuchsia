// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/device-watcher/cpp/device-watcher.h>
#include <lib/fdio/fd.h>
#include <lib/fit/defer.h>
#include <lib/fit/function.h>
#include <lib/sync/cpp/completion.h>

#include <memory>

#include <fbl/ref_ptr.h>
#include <fbl/unique_fd.h>
#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"
#include "src/storage/lib/vfs/cpp/managed_vfs.h"
#include "src/storage/lib/vfs/cpp/pseudo_dir.h"
#include "src/storage/lib/vfs/cpp/pseudo_file.h"

namespace {

TEST(DeviceWatcherTest, Smoke) {
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  auto file = fbl::MakeRefCounted<fs::UnbufferedPseudoFile>(
      [](fbl::String* output) { return ZX_OK; }, [](std::string_view input) { return ZX_OK; });

  auto third = fbl::MakeRefCounted<fs::PseudoDir>();
  ASSERT_OK(third->AddEntry("file", file));

  auto second = fbl::MakeRefCounted<fs::PseudoDir>();
  ASSERT_OK(second->AddEntry("third", std::move(third)));

  auto first = fbl::MakeRefCounted<fs::PseudoDir>();
  ASSERT_OK(first->AddEntry("second", std::move(second)));
  ASSERT_OK(first->AddEntry("file", file));

  auto [client, server] = fidl::Endpoints<fuchsia_io::Directory>::Create();

  fs::ManagedVfs vfs(loop.dispatcher());
  ASSERT_OK(vfs.ServeDirectory(first, std::move(server)));

  ASSERT_OK(loop.StartThread());

  fbl::unique_fd dir;
  ASSERT_OK(fdio_fd_create(client.TakeChannel().release(), dir.reset_and_get_address()));

  ASSERT_OK(device_watcher::RecursiveWaitForFile(dir.get(), "second/third/file").status_value());

  libsync::Completion shutdown_complete;
  vfs.Shutdown([&shutdown_complete](zx_status_t status) {
    EXPECT_OK(status);
    shutdown_complete.Signal();
  });

  shutdown_complete.Wait();
}

TEST(DeviceWatcherTest, OpenInNamespace) {
  ASSERT_OK(device_watcher::RecursiveWaitForFile("/dev/sys/test").status_value());
  ASSERT_STATUS(ZX_ERR_NOT_FOUND,
                device_watcher::RecursiveWaitForFile("/other-test/file").status_value());
}

TEST(DeviceWatcherTest, WatchDirectory) {
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  auto file = fbl::MakeRefCounted<fs::UnbufferedPseudoFile>(
      [](fbl::String* output) { return ZX_OK; }, [](std::string_view input) { return ZX_OK; });
  constexpr char file1_name[] = "file1";
  constexpr char file2_name[] = "file2";
  auto first = fbl::MakeRefCounted<fs::PseudoDir>();
  ASSERT_OK(first->AddEntry(file1_name, file));
  ASSERT_OK(first->AddEntry(file2_name, file));

  auto [client, server] = fidl::Endpoints<fuchsia_io::Directory>::Create();

  fs::ManagedVfs vfs(loop.dispatcher());
  ASSERT_OK(vfs.ServeDirectory(first, std::move(server)));
  auto cleanup = fit::defer([&vfs]() {
    libsync::Completion shutdown_complete;
    vfs.Shutdown([&shutdown_complete](zx_status_t status) {
      EXPECT_OK(status);
      shutdown_complete.Signal();
    });
    shutdown_complete.Wait();
  });

  ASSERT_OK(loop.StartThread());

  std::vector<std::string> file_names;
  zx::result watch_result = device_watcher::WatchDirectoryForItems(
      client, [&file_names](std::string_view file) -> std::optional<std::monostate> {
        file_names.emplace_back(file);
        if (file_names.size() == 2) {
          return std::monostate();
        }
        return std::nullopt;
      });
  ASSERT_OK(watch_result.status_value());

  EXPECT_THAT(file_names,
              testing::UnorderedElementsAre(std::string(file1_name), std::string(file2_name)));
}

TEST(DeviceWatcherTest, WatchDirectoryTemplate) {
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  auto file = fbl::MakeRefCounted<fs::UnbufferedPseudoFile>(
      [](fbl::String* output) { return ZX_OK; }, [](std::string_view input) { return ZX_OK; });
  constexpr char file1_name[] = "file1";
  constexpr char file2_name[] = "file2";
  auto first = fbl::MakeRefCounted<fs::PseudoDir>();
  ASSERT_OK(first->AddEntry(file1_name, file));
  ASSERT_OK(first->AddEntry(file2_name, file));

  auto [client, server] = fidl::Endpoints<fuchsia_io::Directory>::Create();

  fs::ManagedVfs vfs(loop.dispatcher());
  ASSERT_OK(vfs.ServeDirectory(first, std::move(server)));
  auto cleanup = fit::defer([&vfs]() {
    libsync::Completion shutdown_complete;
    vfs.Shutdown([&shutdown_complete](zx_status_t status) {
      EXPECT_OK(status);
      shutdown_complete.Signal();
    });
    shutdown_complete.Wait();
  });

  ASSERT_OK(loop.StartThread());

  zx::result<std::vector<std::string>> watch_result =
      device_watcher::WatchDirectoryForItems<std::vector<std::string>>(
          client,
          [file_names = std::vector<std::string>()](
              std::string_view file) mutable -> std::optional<std::vector<std::string>> {
            file_names.emplace_back(file);
            if (file_names.size() == 2) {
              return std::move(file_names);
            }
            return std::nullopt;
          });
  ASSERT_OK(watch_result.status_value());

  EXPECT_THAT(watch_result.value(),
              testing::UnorderedElementsAre(std::string(file1_name), std::string(file2_name)));
}

TEST(DeviceWatcherTest, DirWatcherWaitForRemoval) {
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  auto file = fbl::MakeRefCounted<fs::UnbufferedPseudoFile>(
      [](fbl::String* output) { return ZX_OK; }, [](std::string_view input) { return ZX_OK; });

  auto third = fbl::MakeRefCounted<fs::PseudoDir>();
  ASSERT_OK(third->AddEntry("file", file));

  auto second = fbl::MakeRefCounted<fs::PseudoDir>();
  ASSERT_OK(second->AddEntry("third", third));

  auto first = fbl::MakeRefCounted<fs::PseudoDir>();
  ASSERT_OK(first->AddEntry("second", second));
  ASSERT_OK(first->AddEntry("file", file));

  auto [client, server] = fidl::Endpoints<fuchsia_io::Directory>::Create();

  fs::ManagedVfs vfs(loop.dispatcher());
  ASSERT_OK(vfs.ServeDirectory(first, std::move(server)));

  ASSERT_OK(loop.StartThread());

  fbl::unique_fd dir;
  ASSERT_OK(fdio_fd_create(client.TakeChannel().release(), dir.reset_and_get_address()));
  fbl::unique_fd sub_dir(openat(dir.get(), "second/third", O_DIRECTORY | O_RDONLY));

  ASSERT_OK(device_watcher::RecursiveWaitForFile(dir.get(), "second/third/file").status_value());

  // Verify removal of the root directory file
  std::unique_ptr<device_watcher::DirWatcher> root_watcher;
  ASSERT_OK(device_watcher::DirWatcher::Create(dir.get(), &root_watcher));

  ASSERT_OK(first->RemoveEntry("file"));
  ASSERT_OK(root_watcher->WaitForRemoval("file", zx::duration::infinite()));

  // Verify removal of the subdirectory file
  std::unique_ptr<device_watcher::DirWatcher> sub_watcher;
  ASSERT_OK(device_watcher::DirWatcher::Create(sub_dir.get(), &sub_watcher));

  ASSERT_OK(third->RemoveEntry("file"));
  ASSERT_OK(sub_watcher->WaitForRemoval("file", zx::duration::infinite()));

  libsync::Completion shutdown_complete;
  vfs.Shutdown([&shutdown_complete](zx_status_t status) {
    EXPECT_OK(status);
    shutdown_complete.Signal();
  });

  shutdown_complete.Wait();
}

TEST(DeviceWatcherTest, DirWatcherVerifyUnowned) {
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  auto file = fbl::MakeRefCounted<fs::UnbufferedPseudoFile>(
      [](fbl::String* output) { return ZX_OK; }, [](std::string_view input) { return ZX_OK; });

  auto first = fbl::MakeRefCounted<fs::PseudoDir>();
  ASSERT_OK(first->AddEntry("file", file));

  auto [client, server] = fidl::Endpoints<fuchsia_io::Directory>::Create();

  fs::ManagedVfs vfs(loop.dispatcher());
  ASSERT_OK(vfs.ServeDirectory(first, std::move(server)));

  ASSERT_OK(loop.StartThread());

  fbl::unique_fd dir;
  ASSERT_OK(fdio_fd_create(client.TakeChannel().release(), dir.reset_and_get_address()));

  std::unique_ptr<device_watcher::DirWatcher> root_watcher;
  ASSERT_OK(device_watcher::DirWatcher::Create(dir.get(), &root_watcher));

  // Close the directory fd
  ASSERT_OK(dir.reset());

  // Verify the watcher can still successfully wait for removal
  ASSERT_OK(first->RemoveEntry("file"));
  ASSERT_OK(root_watcher->WaitForRemoval("file", zx::duration::infinite()));

  libsync::Completion shutdown_complete;
  vfs.Shutdown([&shutdown_complete](zx_status_t status) {
    EXPECT_OK(status);
    shutdown_complete.Signal();
  });

  shutdown_complete.Wait();
}

}  // namespace
