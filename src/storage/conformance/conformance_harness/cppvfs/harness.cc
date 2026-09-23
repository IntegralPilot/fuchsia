// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.io.test/cpp/fidl.h>
#include <fidl/fuchsia.io/cpp/fidl.h>
#include <fidl/test.placeholders/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/result.h>
#include <zircon/status.h>
#include <zircon/types.h>

#include <cstdlib>
#include <memory>

#include <fbl/ref_ptr.h>

#include "src/storage/lib/vfs/cpp/managed_vfs.h"
#include "src/storage/lib/vfs/cpp/pseudo_dir.h"
#include "src/storage/lib/vfs/cpp/remote_dir.h"
#include "src/storage/lib/vfs/cpp/service.h"
#include "src/storage/lib/vfs/cpp/vfs_types.h"
#include "src/storage/lib/vfs/cpp/vmo_file.h"

namespace fio = fuchsia_io;
namespace fio_test = fuchsia_io_test;

class EchoServer : public fidl::Server<test_placeholders::Echo> {
  void EchoString(EchoStringRequest& request, EchoStringCompleter::Sync& completer) override {
    completer.Reply(request.value());
  }
};

class TestHarness : public fidl::Server<fio_test::TestHarness> {
 public:
  explicit TestHarness() : vfs_loop_(&kAsyncLoopConfigNoAttachToCurrentThread) {
    vfs_loop_.StartThread("vfs_thread");
    vfs_ = std::make_unique<fs::ManagedVfs>(vfs_loop_.dispatcher());
  }

  ~TestHarness() override {
    // |fs::ManagedVfs| must be shutdown first before stopping its dispatch loop.
    // Here we asynchronously post the shutdown request, then synchronously join
    // the |vfs_loop_| thread.
    vfs_->Shutdown([this](zx_status_t status) mutable {
      async::PostTask(vfs_loop_.dispatcher(), [this] {
        vfs_.reset();
        vfs_loop_.Quit();
      });
    });
    vfs_loop_.JoinThreads();
  }

  void GetConfig(GetConfigCompleter::Sync& completer) final {
    fio_test::HarnessConfig config;

    // Supported options
    config.supports_get_backing_memory(true);
    config.supports_remote_dir(true);
    config.supports_get_token(true);
    config.supports_mutable_file(true);
    config.supports_services(true);
    config.supports_xattrs(false);
    config.supports_symlinks(false);
    config.supported_attributes(fio::NodeAttributesQuery::kContentSize |
                                fio::NodeAttributesQuery::kStorageSize);

    completer.Reply(config);
  }

  void CreateDirectory(CreateDirectoryRequest& request,
                       CreateDirectoryCompleter::Sync& completer) final {
    fbl::RefPtr<fs::PseudoDir> dir{fbl::MakeRefCounted<fs::PseudoDir>()};
    for (auto& entry : request.contents()) {
      AddEntry(std::move(*entry), *dir);
    }
    zx_status_t status =
        vfs_->Serve(std::move(dir), request.object_request().TakeChannel(), request.flags());
    ZX_ASSERT_MSG(status == ZX_OK, "Serving directory failed: %s", zx_status_get_string(status));
  }

  void OpenServiceDirectory(OpenServiceDirectoryCompleter::Sync& completer) final {
    // Create a directory with a fuchsia.test.placeholders/Echo server at the discoverable name.
    fbl::RefPtr svc_dir = fbl::MakeRefCounted<fs::PseudoDir>();
    auto handler = [this](fidl::ServerEnd<test_placeholders::Echo> server_end) {
      auto instance = std::make_unique<EchoServer>();
      fidl::BindServer(vfs_loop_.dispatcher(), std::move(server_end), std::move(instance));
      return ZX_OK;
    };
    ZX_ASSERT(svc_dir->AddEntry(fidl::DiscoverableProtocolName<test_placeholders::Echo>,
                                fbl::MakeRefCounted<fs::Service>(std::move(handler))) == ZX_OK);
    // Serve it and reply to the request with the client end.
    auto [client_end, server_end] = fidl::Endpoints<fio::Directory>::Create();
    ZX_ASSERT(vfs_->ServeDirectory(std::move(svc_dir), std::move(server_end)) == ZX_OK);
    completer.Reply({std::move(client_end)});
  }

  // NOLINTNEXTLINE(misc-no-recursion): Test-only code, recursion is acceptable here.
  void AddEntry(fio_test::DirectoryEntry entry, fs::PseudoDir& dest) {
    switch (entry.Which()) {
      case fio_test::DirectoryEntry::Tag::kDirectory: {
        fio_test::Directory& directory = entry.directory().value();
        // TODO(https://fxbug.dev/42109125): Set the correct flags on this directory.
        auto dir_entry = fbl::MakeRefCounted<fs::PseudoDir>();
        for (auto& child : directory.entries()) {
          AddEntry(std::move(*child), *dir_entry);
        }
        dest.AddEntry(directory.name(), dir_entry);
        break;
      }
      case fio_test::DirectoryEntry::Tag::kRemoteDirectory: {
        fio_test::RemoteDirectory& remote_dir = entry.remote_directory().value();
        auto remote_entry =
            fbl::MakeRefCounted<fs::RemoteDir>(std::move(remote_dir.remote_client()));
        dest.AddEntry(remote_dir.name(), std::move(remote_entry));
        break;
      }
      case fio_test::DirectoryEntry::Tag::kFile: {
        fio_test::File& file = entry.file().value();
        zx::vmo vmo;
        zx_status_t status = zx::vmo::create(file.contents().size(), {}, &vmo);
        ZX_ASSERT_MSG(status == ZX_OK, "Failed to create VMO: %s", zx_status_get_string(status));
        if (!file.contents().empty()) {
          status = vmo.write(file.contents().data(), 0, file.contents().size());
          ZX_ASSERT_MSG(status == ZX_OK, "Failed to write to VMO: %s",
                        zx_status_get_string(status));
        }
        dest.AddEntry(file.name(),
                      fbl::MakeRefCounted<fs::VmoFile>(std::move(vmo), file.contents().size(),
                                                       /*writable=*/true));
        break;
      }
      case fio_test::DirectoryEntry::Tag::kExecutableFile:
        ZX_PANIC("Executable files are not supported");
        break;
      case fio_test::DirectoryEntry::Tag::kSymlink:
        ZX_PANIC("Symlinks are not supported");
        break;
    }
  }

 private:
  std::unique_ptr<fs::ManagedVfs> vfs_;
  async::Loop vfs_loop_;
};

int main(int argc, const char** argv) {
  fuchsia_logging::LogSettingsBuilder builder;
  builder.WithTags({"io_conformance_harness_cppvfs"}).BuildAndInitialize();

  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  component::OutgoingDirectory outgoing = component::OutgoingDirectory(loop.dispatcher());
  zx::result result = outgoing.ServeFromStartupInfo();
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to serve outgoing directory: " << result.status_string();
    return EXIT_FAILURE;
  }

  result = outgoing.AddProtocol<fio_test::TestHarness>(std::make_unique<TestHarness>());
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to server test harness: " << result.status_string();
    return EXIT_FAILURE;
  }

  return loop.Run();
}
