// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.io.test/cpp/fidl.h>
#include <fidl/fuchsia.io/cpp/fidl.h>
#include <fidl/test.placeholders/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/default.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/vfs/cpp/pseudo_dir.h>
#include <lib/vfs/cpp/pseudo_file.h>
#include <lib/vfs/cpp/remote_dir.h>
#include <lib/vfs/cpp/service.h>
#include <lib/vfs/cpp/vmo_file.h>
#include <zircon/status.h>

#include <memory>
#include <vector>

namespace fio = fuchsia_io;
namespace fio_test = fuchsia_io_test;

class EchoServer : public fidl::Server<test_placeholders::Echo> {
  void EchoString(EchoStringRequest& request, EchoStringCompleter::Sync& completer) override {
    completer.Reply(request.value());
  }
};

class SdkCppHarness : public fidl::Server<fio_test::TestHarness> {
 public:
  explicit SdkCppHarness() = default;

  ~SdkCppHarness() override = default;

  void GetConfig(GetConfigCompleter::Sync& completer) final {
    fio_test::HarnessConfig config;

    // The SDK VFS uses the in-tree C++ VFS under the hood, and thus should support at *least* the
    // same feature set. Other than adding additional supported options, the remainder of this
    // test harness should be the exact same as the current SDK VFS one.

    // Supported options:
    config.supports_get_backing_memory(true);
    config.supports_remote_dir(true);
    config.supports_get_token(true);
    config.supports_mutable_file(true);
    config.supports_services(true);
    config.supports_xattrs(false);
    config.supports_symlinks(false);
    config.supported_attributes(fio::NodeAttributesQuery::kContentSize |
                                fio::NodeAttributesQuery::kStorageSize);

    completer.Reply(std::move(config));
  }

  void CreateDirectory(CreateDirectoryRequest& request,
                       CreateDirectoryCompleter::Sync& completer) final {
    auto dir = std::make_unique<vfs::PseudoDir>();
    for (auto& entry : request.contents()) {
      AddEntry(std::move(*entry), *dir);
    }
    zx_status_t status = dir->Serve(request.flags(), std::move(request.object_request()));
    ZX_ASSERT_MSG(status == ZX_OK, "Failed to serve directory: %s", zx_status_get_string(status));
    // In the SDK VFS, the lifetime of nodes controls connection lifetimes. This means that we must
    // keep the pseudo directory alive until the end of the test.
    directories_.push_back(std::move(dir));
  }

  void OpenServiceDirectory(OpenServiceDirectoryCompleter::Sync& completer) final {
    // Create a directory with a fuchsia.test.placeholders/Echo server at the discoverable name.
    auto svc_dir = std::make_unique<vfs::PseudoDir>();
    auto handler = [](fidl::ServerEnd<test_placeholders::Echo> server_end) {
      auto instance = std::make_unique<EchoServer>();
      fidl::BindServer(async_get_default_dispatcher(), std::move(server_end), std::move(instance));
    };
    ZX_ASSERT(
        svc_dir->AddEntry(std::string(fidl::DiscoverableProtocolName<test_placeholders::Echo>),
                          std::make_unique<vfs::Service>(std::move(handler))) == ZX_OK);
    // Serve it and reply to the request with the client end.
    auto [client_end, server_end] = fidl::Endpoints<fio::Directory>::Create();
    ZX_ASSERT(svc_dir->Serve(fio::wire::kPermReadable, std::move(server_end)) == ZX_OK);
    completer.Reply({std::move(client_end)});
    // Make sure we keep the pseudo-dir alive since it will close all connections when destroyed.
    directories_.push_back(std::move(svc_dir));
  }

 private:
  // NOLINTNEXTLINE(misc-no-recursion): Test-only code, recursion is acceptable here.
  void AddEntry(fio_test::DirectoryEntry entry, vfs::PseudoDir& dest) {
    switch (entry.Which()) {
      case fio_test::DirectoryEntry::Tag::kDirectory: {
        fio_test::Directory directory = std::move(entry.directory().value());
        auto dir_entry = std::make_unique<vfs::PseudoDir>();
        for (auto& child_entry : directory.entries()) {
          AddEntry(std::move(*child_entry), *dir_entry);
        }
        ZX_ASSERT_MSG(dest.AddEntry(directory.name(), std::move(dir_entry)) == ZX_OK,
                      "Failed to add Directory entry!");
        break;
      }
      case fio_test::DirectoryEntry::Tag::kRemoteDirectory: {
        fio_test::RemoteDirectory remote_directory = std::move(entry.remote_directory().value());
        auto remote_dir_entry =
            std::make_unique<vfs::RemoteDir>(std::move(remote_directory.remote_client()));
        dest.AddEntry(remote_directory.name(), std::move(remote_dir_entry));
        break;
      }
      case fio_test::DirectoryEntry::Tag::kFile: {
        fio_test::File file = std::move(entry.file().value());
        zx::vmo vmo;
        zx_status_t status = zx::vmo::create(file.contents().size(), {}, &vmo);
        ZX_ASSERT_MSG(status == ZX_OK, "Failed to create VMO: %s", zx_status_get_string(status));
        if (!file.contents().empty()) {
          status = vmo.write(file.contents().data(), 0, file.contents().size());
          ZX_ASSERT_MSG(status == ZX_OK, "Failed to write to VMO: %s",
                        zx_status_get_string(status));
        }
        auto file_entry = std::make_unique<vfs::VmoFile>(std::move(vmo), file.contents().size(),
                                                         vfs::VmoFile::WriteMode::kWritable);
        ZX_ASSERT_MSG(dest.AddEntry(file.name(), std::move(file_entry)) == ZX_OK,
                      "Failed to add File entry!");
        break;
      }
      case fio_test::DirectoryEntry::Tag::kExecutableFile:
        ZX_PANIC("Executable files are not supported!");
        break;
      case fio_test::DirectoryEntry::Tag::kSymlink:
        ZX_PANIC("Symlinks are not supported!");
        break;
    }
  }

  std::vector<std::unique_ptr<vfs::PseudoDir>> directories_;
};

int main(int argc, const char** argv) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  fuchsia_logging::LogSettingsBuilder builder;
  builder.WithTags({"io_conformance_harness_sdkcpp_new"}).BuildAndInitialize();
  component::OutgoingDirectory outgoing(loop.dispatcher());
  zx::result result =
      outgoing.AddProtocol<fio_test::TestHarness>(std::make_unique<SdkCppHarness>());
  ZX_ASSERT_MSG(result.is_ok(), "Failed to add protocol: %s", result.status_string());
  result = outgoing.ServeFromStartupInfo();
  ZX_ASSERT_MSG(result.is_ok(), "Failed to serve outgoing directory: %s", result.status_string());
  return loop.Run();
}
