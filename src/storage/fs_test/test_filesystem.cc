// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/fs_test/test_filesystem.h"

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/executor.h>
#include <lib/diagnostics/reader/cpp/archive_reader.h>
#include <lib/fdio/cpp/caller.h>
#include <lib/fdio/directory.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/sys/cpp/component_context.h>

#include <gtest/gtest.h>

#include "sdk/lib/syslog/cpp/macros.h"
#include "src/storage/fs_test/crypt_service.h"

namespace fs_test {

using diagnostics::reader::InspectData;

fs_management::MountOptions TestFilesystem::DefaultMountOptions() const {
  fs_management::MountOptions options;
  if (GetTraits().uses_crypt) {
    options.crypt_client = InitializeCryptService;
  }
  return options;
}

zx::result<TestFilesystem> TestFilesystem::FromInstance(
    const TestFilesystemOptions& options, std::unique_ptr<FilesystemInstance> instance) {
  static uint32_t mount_index;
  TestFilesystem filesystem(options, std::move(instance),
                            std::string("/fs_test." + std::to_string(mount_index++) + "/"));
  auto status = filesystem.Mount();
  if (status.is_error()) {
    return status.take_error();
  }
  return zx::ok(std::move(filesystem));
}

zx::result<TestFilesystem> TestFilesystem::Create(const TestFilesystemOptions& options) {
  auto instance_or = options.filesystem->Make(options);
  if (instance_or.is_error()) {
    return instance_or.take_error();
  }
  return FromInstance(options, std::move(instance_or).value());
}

zx::result<TestFilesystem> TestFilesystem::Open(const TestFilesystemOptions& options) {
  auto instance_or = options.filesystem->Open(options);
  if (instance_or.is_error()) {
    return instance_or.take_error();
  }
  return FromInstance(options, std::move(instance_or).value());
}

TestFilesystem::~TestFilesystem() {
  if (filesystem_) {
    if (mounted_) {
      auto status = Unmount();
      if (status.is_error()) {
        FX_LOGS(WARNING) << "Failed to unmount: " << status.status_string() << std::endl;
      }
    }
    rmdir(mount_path_.c_str());
  }
}

zx::result<> TestFilesystem::Mount(const fs_management::MountOptions& options) {
  auto status = filesystem_->Mount(mount_path_, options);
  if (status.is_ok()) {
    mounted_ = true;
  }
  return status;
}

zx::result<> TestFilesystem::Unmount() {
  if (!filesystem_) {
    return zx::ok();
  }
  auto status = filesystem_->Unmount(mount_path_);
  if (status.is_ok()) {
    mounted_ = false;
  }
  return status;
}

zx::result<> TestFilesystem::Fsck() { return filesystem_->Fsck(); }

zx::result<std::string> TestFilesystem::DevicePath() const { return filesystem_->DevicePath(); }

zx::result<fuchsia_io::wire::FilesystemInfo> TestFilesystem::GetFsInfo() const {
  fbl::unique_fd root_fd = fbl::unique_fd(open(mount_path().c_str(), O_RDONLY | O_DIRECTORY));
  fdio_cpp::UnownedFdioCaller root_connection(root_fd);
  const auto& result = fidl::WireCall(fidl::UnownedClientEnd<fuchsia_io::Directory>(
                                          zx::unowned_channel(root_connection.borrow_channel())))
                           ->QueryFilesystem();
  if (!result.ok()) {
    return zx::error(result.status());
  }
  if (result.value().s != ZX_OK) {
    return zx::error(result.value().s);
  }
  return zx::ok(*result.value().info);
}

void TestFilesystem::TakeSnapshot(std::optional<InspectData>* out) const {
  ASSERT_NE(nullptr, out) << "out parameter must be non-null";
  ASSERT_EQ(std::nullopt, *out) << "out parameter will overwrite value already held";

  async::Loop loop = async::Loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  loop.StartThread("inspect-snapshot-thread");
  async::Executor executor(loop.dispatcher());

  std::condition_variable cv;
  std::mutex m;
  bool done = false;

  fpromise::result<std::vector<InspectData>, std::string> data_or_err;
  auto component_selector =
      diagnostics::reader::SanitizeMonikerForSelectors(filesystem_->GetMoniker());
  {
    diagnostics::reader::ArchiveReader reader(executor.dispatcher(),
                                              {component_selector + ":root"});
    auto promise =
        reader.SnapshotInspectUntilPresent({filesystem_->GetMoniker()})
            .then([&](fpromise::result<std::vector<InspectData>, std::string>& inspect_data) {
              {
                std::unique_lock<std::mutex> lock(m);
                data_or_err = std::move(inspect_data);
                done = true;
              }
              cv.notify_all();
            });

    executor.schedule_task(std::move(promise));

    std::unique_lock<std::mutex> lock(m);
    cv.wait(lock, [&done]() { return done; });
  }
  loop.Quit();
  loop.JoinThreads();

  ASSERT_TRUE(data_or_err.is_ok())
      << "Failed to obtain inspect tree snapshot: " << data_or_err.take_error();
  auto all = data_or_err.take_value();
  ASSERT_EQ(all.size(), 1ul) << "There should be exactly one matching Inspect hierarchy";
  *out = {std::move(all[0])};
}

}  // namespace fs_test
