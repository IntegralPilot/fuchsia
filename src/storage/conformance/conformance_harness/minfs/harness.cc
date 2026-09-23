// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.io.test/cpp/fidl.h>
#include <fidl/fuchsia.io/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>
#include <zircon/status.h>
#include <zircon/types.h>

#include <cstdint>
#include <memory>
#include <string>
#include <vector>

#include <fbl/ref_ptr.h>

#include "src/storage/lib/block_client/cpp/fake_block_device.h"
#include "src/storage/lib/vfs/cpp/vfs_types.h"
#include "src/storage/minfs/bcache.h"
#include "src/storage/minfs/directory.h"
#include "src/storage/minfs/format.h"
#include "src/storage/minfs/minfs.h"
#include "src/storage/minfs/minfs_private.h"
#include "src/storage/minfs/mount.h"
#include "src/storage/minfs/runner.h"
#include "src/storage/minfs/vnode.h"

namespace fio = fuchsia_io;
namespace fio_test = fuchsia_io_test;

namespace minfs {

constexpr uint64_t kBlockCount = 1 << 13;

class MinfsHarness : public fidl::Server<fio_test::TestHarness> {
 public:
  explicit MinfsHarness() : vfs_loop_(&kAsyncLoopConfigNoAttachToCurrentThread) {
    vfs_loop_.StartThread("vfs_thread");

    auto device = std::make_unique<block_client::FakeBlockDevice>(kBlockCount, kMinfsBlockSize);

    auto bcache = Bcache::Create(std::move(device), kBlockCount);
    ZX_ASSERT(bcache.is_ok());
    // TODO(https://fxbug.dev/375550868): should not be overriding inode count
    auto mount_options = MountOptions{.inode_count = 8192};
    ZX_ASSERT(Mkfs(mount_options, bcache.value().get()).is_ok());

    auto runner = Runner::Create(vfs_loop_.dispatcher(), *std::move(bcache), {});
    ZX_ASSERT(runner.is_ok());
    runner_ = *std::move(runner);

    // One connection must be maintained to avoid filesystem termination.
    auto [root_client, root_server] = fidl::Endpoints<fio::Directory>::Create();
    root_client_ = std::move(root_client);
    zx::result status = runner_->ServeRoot(std::move(root_server));
    ZX_ASSERT(status.is_ok());
  }

  ~MinfsHarness() override {
    // The runner shutdown takes care of shutting everything down in the right order, including the
    // async loop.
    runner_->Shutdown([](zx_status_t status) { ZX_ASSERT(status == ZX_OK); });
    vfs_loop_.JoinThreads();
  }

  void GetConfig(GetConfigCompleter::Sync& completer) final {
    fio_test::HarnessConfig config;

    // Supported options
    config.supports_get_token(true);
    config.supports_append(true);
    config.supports_truncate(true);
    config.supports_modify_directory(true);
    config.supports_mutable_file(true);
    config.supports_xattrs(false);
    config.supports_symlinks(false);
    config.supported_attributes(
        fio::NodeAttributesQuery::kCreationTime | fio::NodeAttributesQuery::kModificationTime |
        fio::NodeAttributesQuery::kId | fio::NodeAttributesQuery::kContentSize |
        fio::NodeAttributesQuery::kStorageSize | fio::NodeAttributesQuery::kLinkCount);

    completer.Reply(config);
  }

  void CreateDirectory(CreateDirectoryRequest& request,
                       CreateDirectoryCompleter::Sync& completer) final {
    // Create a unique directory within the root of minfs for each request and populate it with the
    // requested contents.
    auto directory = CreateUniqueDirectory();
    PopulateDirectory(request.contents(), *directory);
    zx_status_t status = runner_->Serve(std::move(directory),
                                        request.object_request().TakeChannel(), request.flags());
    ZX_ASSERT_MSG(status == ZX_OK, "Failed to serve test directory: %s",
                  zx_status_get_string(status));
  }

  void OpenServiceDirectory(OpenServiceDirectoryCompleter::Sync& completer) final {
    ZX_PANIC("Not supported.");
  }

  // NOLINTNEXTLINE(misc-no-recursion): Test-only code, recursion is acceptable here.
  void PopulateDirectory(const std::vector<fidl::Box<fio_test::DirectoryEntry>>& entries,
                         Directory& dir) {
    for (const auto& entry : entries) {
      AddEntry(*entry, dir);
    }
  }

  // NOLINTNEXTLINE(misc-no-recursion): Test-only code, recursion is acceptable here.
  void AddEntry(const fio_test::DirectoryEntry& entry, Directory& parent) {
    switch (entry.Which()) {
      case fio_test::DirectoryEntry::Tag::kDirectory: {
        zx::result vnode = parent.Create(entry.directory()->name(), fs::CreationType::kDirectory);
        ZX_ASSERT_MSG(vnode.is_ok(), "Failed to create a directory: %s", vnode.status_string());
        auto directory = fbl::RefPtr<Directory>::Downcast(*std::move(vnode));
        ZX_ASSERT_MSG(directory != nullptr, "A vnode of the wrong type was created");
        PopulateDirectory(entry.directory()->entries(), *directory);
        // The directory was opened when it was created.
        directory->Close();
        break;
      }
      case fio_test::DirectoryEntry::Tag::kFile: {
        zx::result file = parent.Create(entry.file()->name(), fs::CreationType::kFile);
        ZX_ASSERT_MSG(file.is_ok(), "Failed to create a file: %s", file.status_string());
        const auto& contents = entry.file()->contents();
        if (!contents.empty()) {
          size_t actual = 0;
          zx_status_t status = file->Write(contents.data(), contents.size(),
                                           /*offset=*/0, &actual);
          ZX_ASSERT_MSG(status == ZX_OK, "Failed to write to file: %s",
                        zx_status_get_string(status));
        }
        // The file was opened when it was created.
        file->Close();
        break;
      }
      case fio_test::DirectoryEntry::Tag::kRemoteDirectory:
        ZX_PANIC("Remote directories are not supported");
      case fio_test::DirectoryEntry::Tag::kExecutableFile:
        ZX_PANIC("Executable files are not supported");
      case fio_test::DirectoryEntry::Tag::kSymlink:
        ZX_PANIC("Symlinks are not supported");
    }
  }

  fbl::RefPtr<Directory> GetRootNode() {
    auto vn_or = runner_->minfs().VnodeGet(kMinfsRootIno);
    ZX_ASSERT(vn_or.is_ok());
    auto root = fbl::RefPtr<Directory>::Downcast(std::move(vn_or.value()));
    ZX_ASSERT_MSG(root != nullptr, "The root node wasn't a directory");
    return root;
  }

  fbl::RefPtr<Directory> CreateUniqueDirectory() {
    ++directory_count_;
    std::string directory_name = std::to_string(directory_count_);
    fbl::RefPtr<Directory> root = GetRootNode();
    zx::result vnode = root->Create(directory_name, fs::CreationType::kDirectory);
    ZX_ASSERT_MSG(vnode.is_ok(), "Failed to create a unique directory: %s", vnode.status_string());
    auto directory = fbl::RefPtr<Directory>::Downcast(*std::move(vnode));
    ZX_ASSERT_MSG(directory != nullptr, "A vnode of the wrong type was created");
    // The directory was opened when it was created.
    directory->Close();
    return directory;
  }

 private:
  async::Loop vfs_loop_;
  std::unique_ptr<Runner> runner_;

  // Used to create a new unique directory within minfs for every call to |CreateDirectory|.
  uint32_t directory_count_ = 0;
  fidl::ClientEnd<fio::Directory> root_client_;
};

}  // namespace minfs

int main(int argc, const char** argv) {
  fuchsia_logging::LogSettingsBuilder builder;
  builder.WithTags({"io_conformance_harness_minfs"}).BuildAndInitialize();

  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  component::OutgoingDirectory outgoing = component::OutgoingDirectory(loop.dispatcher());
  zx::result result = outgoing.ServeFromStartupInfo();
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to serve outgoing directory: " << result.status_string();
    return EXIT_FAILURE;
  }

  result = outgoing.AddProtocol<fio_test::TestHarness>(std::make_unique<minfs::MinfsHarness>());
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to server test harness: " << result.status_string();
    return EXIT_FAILURE;
  }

  loop.Run();
  return EXIT_SUCCESS;
}
