// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <errno.h>
#include <fidl/fuchsia.boot/cpp/markers.h>
#include <fidl/fuchsia.boot/cpp/wire.h>
#include <fidl/fuchsia.debugdata/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/stdcompat/array.h>
#include <lib/stdcompat/source_location.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/vfs/cpp/pseudo_dir.h>
#include <lib/vfs/cpp/service.h>
#include <lib/vfs/cpp/vmo_file.h>
#include <lib/zbi-format/internal/debugdata.h>
#include <lib/zbi-format/zbi.h>
#include <lib/zbitl/item.h>
#include <zircon/status.h>
#include <zircon/syscalls/object.h>

#include <span>
#include <vector>

#include <fbl/unique_fd.h>

#include "src/lib/fxl/command_line.h"
#include "src/lib/fxl/log_settings_command_line.h"

// This a test component, with the sole job of providing a fake /boot and /svc to its parent, who
// later will reroute it to its child.

namespace {

class ProviderServer final : public fidl::WireServer<fuchsia_boot::SvcStashProvider> {
 public:
  ProviderServer(async_dispatcher_t* dispatcher,
                 fidl::ServerEnd<fuchsia_boot::SvcStashProvider> server_end)
      : binding_(fidl::BindServer(dispatcher, std::move(server_end), this)) {
    zx::result stash_client_end = fidl::CreateEndpoints(&stash_);
    if (stash_client_end.is_error()) {
      FX_PLOGS(ERROR, stash_client_end.status_value()) << "Failed to create stash endpoints";
      return;
    }
    fidl::WireSyncClient stash_client(std::move(stash_client_end.value()));

    zx::result directory_server_end = fidl::CreateEndpoints(&svc_);
    if (directory_server_end.is_error()) {
      FX_PLOGS(ERROR, directory_server_end.status_value())
          << "Failed to create directory endpoints";
      return;
    }

    const fidl::OneWayStatus status = stash_client->Store(std::move(directory_server_end.value()));
    if (!status.ok()) {
      FX_PLOGS(ERROR, status.status()) << "Failed to store directory in stash";
      return;
    }

    // Publish data to publisher_client.
    zx::result publisher_client_end = component::ConnectAt<fuchsia_debugdata::Publisher>(svc_);
    if (publisher_client_end.is_error()) {
      FX_PLOGS(ERROR, publisher_client_end.status_value()) << "Failed to connect to publisher";
      return;
    }
    fidl::WireSyncClient publisher(std::move(publisher_client_end.value()));

    constexpr std::string_view kSink = "llvm-profile";
    constexpr std::string_view kCustomSink = "my-custom-sink";
    for (const auto& [name, vmo_content, data_sink, is_dynamic] :
         (std::tuple<std::string_view, std::string_view, std::string_view, bool>[]){
             {"profraw", "1234", kSink, false},      // llvm-profile static
             {"profraw", "567890123", kSink, true},  // llvm-profile dynamic
             {"custom", "789", kCustomSink, false},  // custom static
             {{}, "43218765", kCustomSink, true},    // custom dynamic
         }) {
      zx::vmo vmo;
      if (zx_status_t status = zx::vmo::create(4096, 0, &vmo); status != ZX_OK) {
        FX_PLOGS(ERROR, status) << "Failed to create vmo for " << name;
        return;
      }

      if (!name.empty()) {
        if (zx_status_t status = vmo.set_property(ZX_PROP_NAME, name.data(), name.size());
            status != ZX_OK) {
          FX_PLOGS(ERROR, status) << "Failed to set vmo name for " << name;
          return;
        }
      }

      if (zx_status_t status = vmo.write(vmo_content.data(), 0, vmo_content.size());
          status != ZX_OK) {
        FX_PLOGS(ERROR, status) << "Failed to write publisher data for " << name;
        return;
      }

      zx::eventpair token_1, token_2;
      if (zx_status_t status = zx::eventpair::create(0, &token_1, &token_2); status != ZX_OK) {
        FX_PLOGS(ERROR, status) << "Failed to create eventpair for " << name;
        return;
      }
      if (is_dynamic) {
        tokens_.push_back(std::move(token_1));
      }

      const fidl::OneWayStatus status = publisher->Publish(
          fidl::StringView::FromExternal(data_sink), std::move(vmo), std::move(token_2));
      if (!status.ok()) {
        FX_PLOGS(ERROR, status.status()) << "Failed to publish " << name;
        return;
      }
    }
  }

  void Get(GetCompleter::Sync& completer) final { completer.ReplySuccess(std::move(stash_)); }

 private:
  fidl::ServerBindingRef<fuchsia_boot::SvcStashProvider> binding_;
  fidl::ServerEnd<fuchsia_boot::SvcStash> stash_;
  fidl::ClientEnd<fuchsia_io::Directory> svc_;

  std::vector<zx::eventpair> tokens_;
};

struct VmoInfo {
  zx::vmo vmo;
  uint32_t len;
};

void AddDebugDataItem(std::string_view name, std::string_view sink, std::string_view contents,
                      std::string_view logs, std::vector<VmoInfo>& dd_vmos) {
  const zbi_debugdata_t trailer = {
      .content_size = static_cast<uint32_t>(contents.size()),
      .sink_name_size = static_cast<uint32_t>(sink.size()),
      .vmo_name_size = static_cast<uint32_t>(name.size()),
      .log_size = static_cast<uint32_t>(logs.size()),
  };
  size_t vmo_size =
      sizeof(trailer) + zbitl::AlignedPayloadLength(static_cast<uint32_t>(
                            contents.size() + sink.size() + name.size() + logs.size()));
  std::vector<char> dd_content = {};
  dd_content.resize(vmo_size);
  std::string_view trailer_view{reinterpret_cast<const char*>(&trailer), sizeof(trailer)};
  size_t offset = 0;
  for (const auto& content : std::to_array<std::string_view>({contents, sink, name, logs})) {
    content.copy(dd_content.data() + offset, content.size());
    offset += content.size();
  }
  trailer_view.copy(dd_content.data() + dd_content.size() - trailer_view.size(),
                    trailer_view.size());

  zx::vmo debug_data_vmo;
  ZX_ASSERT(zx::vmo::create(vmo_size, 0, &debug_data_vmo) == ZX_OK);
  ZX_ASSERT(debug_data_vmo.write(dd_content.data(), 0, dd_content.size()) == ZX_OK);
  dd_vmos.push_back({.vmo = std::move(debug_data_vmo), .len = static_cast<uint32_t>(vmo_size)});
}

// We need to fake a fuchsia.boot.Items FIDL server.
class FakeFuchsiaBootItemServer : public fidl::WireServer<fuchsia_boot::Items> {
 public:
  explicit FakeFuchsiaBootItemServer(async_dispatcher_t* dispatcher,
                                     fidl::ServerEnd<fuchsia_boot::Items> server_end,
                                     std::span<const VmoInfo> dd_vmos)
      : dd_vmos_(dd_vmos) {
    fidl::BindServer(dispatcher, std::move(server_end), this);
  }

  void Get(fuchsia_boot::wire::ItemsGetRequest* request, GetCompleter::Sync& completer) final {
    FX_LOGS(ERROR) << "Unexpected FIDL call to `fuchsia.boot.Items::Get`";
  }

  void Get2(fuchsia_boot::wire::ItemsGet2Request* request, Get2Completer::Sync& completer) final {
    if (request->type != ZBI_TYPE_DEBUGDATA) {
      completer.ReplyError(ZX_ERR_NOT_FOUND);
      return;
    }
    std::vector<fuchsia_boot::wire::RetrievedItems> vmos_to_return;
    for (const auto& vmo_info : dd_vmos_) {
      zx::vmo dup;
      if (auto res = vmo_info.vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup); res != ZX_OK) {
        completer.ReplyError(res);
        return;
      }
      vmos_to_return.push_back({
          .payload = std::move(dup),
          .length = vmo_info.len,

      });
    }
    completer.ReplySuccess(
        fidl::VectorView<fuchsia_boot::wire::RetrievedItems>::FromExternal(vmos_to_return));
  }

  void GetBootloaderFile(fuchsia_boot::wire::ItemsGetBootloaderFileRequest* request,
                         GetBootloaderFileCompleter::Sync& completer) final {
    FX_LOGS(ERROR) << "Unexpected FIDL call to `fuchsia.boot.Items::GetBootloaderFile`";
  }

 private:
  std::span<const VmoInfo> dd_vmos_;
};

}  // namespace

int main(int argc, char** argv) {
  const auto command_line = fxl::CommandLineFromArgcArgv(argc, argv);
  fxl::SetLogSettingsFromCommandLine(command_line,
                                     {"early-boot-instrumentation", "capability-provider"});

  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  auto context = sys::ComponentContext::CreateAndServeOutgoingDirectory();

  std::vector<VmoInfo> dd_vmos;
  AddDebugDataItem("foo", "bar", "foozbarz", "", dd_vmos);
  AddDebugDataItem("fooz", "bar", "foozbarz", "Fooz Barz", dd_vmos);
  AddDebugDataItem("foo", "llvm-profile", "foozbarz", "Fooz Barz", dd_vmos);

  // Bind the Provider server.
  std::vector<std::unique_ptr<ProviderServer>> connections;
  auto provider_svc = std::make_unique<vfs::Service>(
      [&connections](zx::channel request, async_dispatcher_t* dispatcher) {
        fidl::ServerEnd<fuchsia_boot::SvcStashProvider> server_end(std::move(request));
        auto connection = std::make_unique<ProviderServer>(dispatcher, std::move(server_end));
        connections.push_back(std::move(connection));
      });
  if (context->outgoing()->AddPublicService(
          std::move(provider_svc),
          fidl::DiscoverableProtocolName<fuchsia_boot::SvcStashProvider>) != ZX_OK) {
    FX_LOGS(ERROR) << "Failed to add provider servicer.";
  }
  std::vector<std::unique_ptr<FakeFuchsiaBootItemServer>> boot_item_connections;
  auto boot_items_svc = std::make_unique<vfs::Service>(
      [&boot_item_connections, &dd_vmos](zx::channel request, async_dispatcher_t* dispatcher) {
        fidl::ServerEnd<fuchsia_boot::Items> server_end(std::move(request));
        auto connection =
            std::make_unique<FakeFuchsiaBootItemServer>(dispatcher, std::move(server_end), dd_vmos);
        boot_item_connections.push_back(std::move(connection));
      });
  if (context->outgoing()->AddPublicService(std::move(boot_items_svc),
                                            fidl::DiscoverableProtocolName<fuchsia_boot::Items>) !=
      ZX_OK) {
    FX_LOGS(ERROR) << "Failed to add provider servicer.";
  }
  // Add prof_data dir.
  auto* boot = context->outgoing()->GetOrCreateDirectory("boot");
  auto debugdata = std::make_unique<vfs::PseudoDir>();
  auto logs = std::make_unique<vfs::PseudoDir>();
  auto kernel = std::make_unique<vfs::PseudoDir>();
  auto boot_llvm_sink = std::make_unique<vfs::PseudoDir>();
  auto llvm_static = std::make_unique<vfs::PseudoDir>();
  auto llvm_dynamic = std::make_unique<vfs::PseudoDir>();

  // Fake Kernel vmo.
  zx::vmo kernel_vmo;
  if (auto res = zx::vmo::create(4096, 0, &kernel_vmo); res != ZX_OK) {
    FX_LOGS(ERROR) << "Failed to create Kernel VMO. Error: " << zx_status_get_string(res);
  } else {
    if (auto res = kernel_vmo.write("kernel", 0, 7); res != ZX_OK) {
      FX_LOGS(ERROR) << "Failed to write Kernel VMO contents. Error: " << zx_status_get_string(res);
    }
    auto kernel_file = std::make_unique<vfs::VmoFile>(std::move(kernel_vmo), 4096);
    llvm_dynamic->AddEntry("zircon.profraw", std::move(kernel_file));
  }

  // Fake Physboot VMO.
  zx::vmo phys_vmo;
  if (auto res = zx::vmo::create(4096, 0, &phys_vmo); res != ZX_OK) {
    FX_LOGS(ERROR) << "Failed to create Physboot VMO. Error: " << zx_status_get_string(res);
  } else {
    if (auto res = phys_vmo.write("physboot", 0, 9); res != ZX_OK) {
      FX_LOGS(ERROR) << "Failed to write Kernel VMO contents. Error: " << zx_status_get_string(res);
    }
    auto physboot_file = std::make_unique<vfs::VmoFile>(std::move(phys_vmo), 4096);
    llvm_static->AddEntry("physboot.profraw", std::move(physboot_file));
  }

  // Fake Log VMO.
  auto entries = cpp20::to_array<std::string_view>(
      {"symbolizer.log", "physboot.log", "physload.log", "foo-bar.log"});
  for (auto entry : entries) {
    zx::vmo logs_vmo;
    if (auto res = zx::vmo::create(4096, 0, &logs_vmo); res != ZX_OK) {
      FX_LOGS(ERROR) << "Failed to create " << entry
                     << " VMO. Error: " << zx_status_get_string(res);
    } else {
      if (auto res = logs_vmo.write(entry.data(), 0, entry.length()); res != ZX_OK) {
        FX_LOGS(ERROR) << "Failed to writes " << entry
                       << " vmo contents. Error: " << zx_status_get_string(res);
      }
      auto logs_file = std::make_unique<vfs::VmoFile>(std::move(logs_vmo), 4096);
      logs->AddEntry(std::string(entry), std::move(logs_file));
    }
  }

  boot_llvm_sink->AddEntry("s", std::move(llvm_static));
  boot_llvm_sink->AddEntry("d", std::move(llvm_dynamic));
  debugdata->AddEntry("logs", std::move(logs));
  debugdata->AddEntry("llvm-profile", std::move(boot_llvm_sink));
  kernel->AddEntry("i", std::move(debugdata));
  boot->AddEntry("kernel", std::move(kernel));
  // boot/kernel/i/llvm-profile/{s,d}/{vmos}
  loop.Run();
  return 0;
}
