// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <fidl/fuchsia.buildinfo/cpp/wire.h>
#include <fidl/fuchsia.fshost/cpp/wire.h>
#include <fidl/fuchsia.hardware.power.statecontrol/cpp/natural_ostream.h>
#include <fidl/fuchsia.paver/cpp/wire.h>
#include <lib/async/default.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fastboot/fastboot.h>
#include <lib/fdio/directory.h>
#include <lib/zx/result.h>
#include <zircon/status.h>

#include <chrono>
#include <condition_variable>
#include <mutex>
#include <optional>
#include <string_view>
#include <thread>
#include <utility>
#include <vector>

#include <fbl/unique_fd.h>
#include <sdk/lib/syslog/cpp/macros.h>

#include "src/firmware/lib/fastboot/rust/ffi_c/bindings.h"
#include "src/firmware/lib/fastboot/sparse_format.h"
#include "src/lib/fxl/strings/split_string.h"
#include "src/lib/fxl/strings/string_printf.h"

namespace fastboot {
namespace {

using fuchsia_hardware_power_statecontrol::wire::ShutdownAction;
using fuchsia_hardware_power_statecontrol::wire::ShutdownOptions;
using fuchsia_hardware_power_statecontrol::wire::ShutdownReason;

constexpr char kFastbootLogTag[] = __FILE__;

constexpr auto kShutdownDelay = std::chrono::seconds(1);

constexpr uint64_t kFillBufferNumPages = 1ull;

/// Suffix added to the product if it happens to match the name of the board. This prevents issues
/// when trying to flash a product via https://flash.android.com/ since it currently relies on the
/// values of `product` and `hw-revision` to be unique.
constexpr char kUniqueProductSuffix = '-';

struct FlashPartitionInfo {
  std::string_view partition;
  std::optional<fuchsia_paver::wire::Configuration> configuration;
};

FlashPartitionInfo GetPartitionInfo(std::string_view partition_label) {
  size_t len = partition_label.length();
  if (len < 2) {
    return {.partition = partition_label, .configuration = std::nullopt};
  }

  FlashPartitionInfo ret;
  ret.partition = partition_label.substr(0, len - 2);
  std::string_view slot_suffix = partition_label.substr(len - 2, 2);
  // TODO(b/241150035): Some platforms such as x64 still use legacy kernel partition name
  // zircon-a/b/r. Hardcode these cases for backward compatibility. Once all products migrate to new
  // name. Remove them.
  if (slot_suffix == "_a" || partition_label == "zircon-a") {
    ret.configuration = fuchsia_paver::wire::Configuration::kA;
  } else if (slot_suffix == "_b" || partition_label == "zircon-b") {
    ret.configuration = fuchsia_paver::wire::Configuration::kB;
  } else if (slot_suffix == "_r" || partition_label == "zircon-r") {
    ret.configuration = fuchsia_paver::wire::Configuration::kRecovery;
  } else {
    ret.partition = partition_label;
  }

  return ret;
}

// NOLINTNEXTLINE(modernize-avoid-variadic-functions): This is passed to a C library for logging.
int LogUnsparseError(const char* format, ...) {
  va_list(args);
  va_start(args, format);
  constexpr size_t kMaxLogMessageSize = 4096;
  char buff[kMaxLogMessageSize];
  int len = vsnprintf(&buff[0], kMaxLogMessageSize, format, args);
  if (len > 0 && std::cmp_less(len, kMaxLogMessageSize)) {
    std::string_view message(&buff[0], len);
    FX_LOGST(ERROR, kFastbootLogTag) << "Error unsparsing payload: " << buff;
  } else {
    FX_LOGST(ERROR, kFastbootLogTag) << "Failed to format log message from sparse library.";
  }
  return 0;
}

}  // namespace

const std::vector<Fastboot::CommandEntry>& Fastboot::GetCommandTable() {
  // Using a static pointer and allocate with `new` so that the static instance
  // never gets deleted.

  static const std::vector<CommandEntry>* kCommandTable = new std::vector<CommandEntry>({
      {
          .name = "getvar",
          .cmd = &Fastboot::GetVar,
      },
      {
          .name = "flash",
          .cmd = &Fastboot::Flash,
      },
      {
          .name = "erase",
          .cmd = &Fastboot::Erase,
      },
      {
          .name = "set_active",
          .cmd = &Fastboot::SetActive,
      },
      {
          .name = "continue",
          .cmd = &Fastboot::Continue,
      },
      {
          .name = "reboot",
          .cmd = &Fastboot::Reboot,
      },
      {
          .name = "reboot-bootloader",
          .cmd = &Fastboot::RebootBootloader,
      },
      {
          .name = "reboot-fastboot",
          .cmd = &Fastboot::RebootFastboot,
      },
      {
          .name = "reboot-recovery",
          .cmd = &Fastboot::RebootRecovery,
      },
      {
          .name = "oem add-staged-bootloader-file",
          .cmd = &Fastboot::OemAddStagedBootloaderFile,
      },
      {
          .name = "oem init-partition-tables",
          .cmd = &Fastboot::OemInitPartitionTables,
      },
      {
          .name = "oem install-from-usb",
          .cmd = &Fastboot::OemInstallFromUsb,
      },
      {
          .name = "oem wipe-partition-tables",
          .cmd = &Fastboot::OemWipePartitionTables,
      },
      {
          .name = "oem install-blob-image",
          .cmd = &Fastboot::OemInstallBlobImage,
      },
      {
          .name = "update-super",
          .cmd = &Fastboot::UpdateSuper,
      },
  });
  return *kCommandTable;
}

const Fastboot::VariableHashTable& Fastboot::GetVariableTable() {
  // Using a static pointer and allocate with `new` so that the static instance
  // never gets deleted.
  static const VariableHashTable* kVariableTable = new VariableHashTable({
      {"max-download-size", &Fastboot::GetVarMaxDownloadSize},
      {"slot-count", &Fastboot::GetVarSlotCount},
      {"is-userspace", &Fastboot::GetVarIsUserspace},
      {"hw-revision", &Fastboot::GetVarHwRevision},
      {"product", &Fastboot::GetVarProduct},
      {"version", &Fastboot::GetVarVersion},
      {"all", &Fastboot::GetVarAll},
  });
  return *kVariableTable;
}

Fastboot::Fastboot() {
  // Allocate up to 2% of total memory for the fastboot download buffer.
  //
  // We may need to tweak this further; low-memory devices need to be conservative here to avoid
  // OOMing the system, but a smaller buffer results in slower downloads since images need to be
  // split into more chunks which creates more overhead.
  //
  // The VMO will only be instantiated when a download is requested, and will be released when the
  // download is finished.
  max_download_size_ = zx_system_get_physmem() / 50;
}

Fastboot::Fastboot(size_t max_download_size, fidl::ClientEnd<fuchsia_io::Directory> svc_root)
    : max_download_size_(max_download_size), svc_root_(std::move(svc_root)) {}

zx::result<> Fastboot::ProcessCommand(std::string_view command, Transport* transport) {
  for (const CommandEntry& cmd : GetCommandTable()) {
    if (MatchCommand(command, cmd.name)) {
      return (this->*cmd.cmd)(command.data(), transport);
    }
  }

  return SendResponse(ResponseType::kFail,
                      std::string("Unsupported command: ") + std::string(command), transport);
}

void Fastboot::DoClearDownload() { download_vmo_mapper_.Reset(); }

zx::result<void*> Fastboot::GetDownloadBuffer(size_t total_download_size) {
  if (zx_status_t ret = download_vmo_mapper_.CreateAndMap(total_download_size, "fastboot download");
      ret != ZX_OK) {
    return zx::error(ret);
  }

  if (zx_status_t ret = download_vmo_mapper_.vmo().set_prop_content_size(total_download_size);
      ret != ZX_OK) {
    return zx::error(ret);
  }

  return zx::ok(download_vmo_mapper_.start());
}

zx::result<> Fastboot::GetVar(const std::string& command, Transport* transport) {
  std::vector<std::string_view> args =
      fxl::SplitString(command, ":", fxl::kTrimWhitespace, fxl::kSplitWantNonEmpty);
  if (args.size() < 2) {
    return SendResponse(ResponseType::kFail, "Not enough arguments", transport);
  }

  // TODO(https://fxbug.dev/397515768): We should avoid hard-coding partition types/sizes here.
  // This is a temporary implementation to unblock fastboot -w support. Hard-coding these here means
  // we won't see these variables with `fastboot getvar all`, but the current variable table doesn't
  // support a means of querying multi-part variables.
  if (args.size() >= 3) {
    if (args[1] == "partition-type" && args[2] == "userdata") {
      return SendResponse(ResponseType::kOkay, "fxfs-vol", transport);
    }
    if (args[1] == "partition-size" && args[2] == "userdata") {
      return SendResponse(ResponseType::kOkay, "0x00", transport);
    }
  }

  const VariableHashTable& var_table = GetVariableTable();
  const VariableHashTable::const_iterator find_res = var_table.find(args[1].data());
  if (find_res == var_table.end()) {
    return SendResponse(ResponseType::kFail, "Unknown variable", transport);
  }

  zx::result<std::string> var = (this->*(find_res->second))(args, transport);
  if (var.is_error()) {
    return SendResponse(ResponseType::kFail, "Failed to get variable", transport,
                        zx::error(var.status_value()));
  }

  return SendResponse(ResponseType::kOkay, *var, transport);
}

zx::result<std::string> Fastboot::GetVarVersion(const std::vector<std::string_view>&, Transport*) {
  return zx::ok("0.4");
}

zx::result<std::string> Fastboot::GetVarAll(const std::vector<std::string_view>& args,
                                            Transport* transport) {
  zx::result<std::string> res(zx::ok("done"));

  const VariableHashTable& var_table = GetVariableTable();
  for (const auto& [name, func] : var_table) {
    if (name == "all") {
      continue;
    }

    std::vector<std::string_view> var_args = {"getvar", name};
    zx::result<std::string> var_ret = (this->*(func))(var_args, transport);

    if (var_ret.is_error()) {
      res = zx::ok("not all variables were retrieved successfully");
    }

    std::string response = fxl::StringPrintf(
        "%s: %s", name.c_str(),
        var_ret.value_or(std::string("[error: ") + var_ret.status_string() + "]").c_str());

    zx::result<> ret = SendResponse(ResponseType::kInfo, response, transport);
    if (ret.is_error()) {
      return zx::error(ret.status_value());
    }
  }

  return res;
}

zx::result<std::string> Fastboot::GetVarMaxDownloadSize(const std::vector<std::string_view>&,
                                                        Transport*) {
  return zx::ok(fxl::StringPrintf("0x%08zx", max_download_size_));
}

zx::result<std::string> Fastboot::GetVarHwRevision(const std::vector<std::string_view>&,
                                                   Transport*) {
  auto svc_root = GetSvcRoot();
  if (svc_root.is_error()) {
    return zx::error(svc_root.status_value());
  }
  auto provider = component::ConnectAt<fuchsia_buildinfo::Provider>(*svc_root);
  if (provider.is_error()) {
    return zx::error(provider.status_value());
  }
  auto build_info = fidl::WireCall(*provider)->GetBuildInfo();
  if (!build_info.ok()) {
    return zx::error(build_info.status());
  }
  return zx::ok(build_info->build_info.board_config().data());
}

zx::result<std::string> Fastboot::GetVarProduct(const std::vector<std::string_view>&, Transport*) {
  auto svc_root = GetSvcRoot();
  if (svc_root.is_error()) {
    return zx::error(svc_root.status_value());
  }
  auto provider = component::ConnectAt<fuchsia_buildinfo::Provider>(*svc_root);
  if (provider.is_error()) {
    return zx::error(provider.status_value());
  }
  auto response = fidl::WireCall(*provider)->GetBuildInfo();
  if (!response.ok()) {
    return zx::error(response.status());
  }
  auto build_info = response->build_info;
  std::string_view product_config(build_info.product_config().data(),
                                  build_info.product_config().size());
  std::string_view board_config(build_info.board_config().data(), build_info.board_config().size());
  if (product_config != board_config) {
    return zx::ok(product_config);
  }
  std::string product(product_config);
  product.push_back(kUniqueProductSuffix);
  return zx::ok(product);
}

zx::result<std::string> Fastboot::GetVarSlotCount(const std::vector<std::string_view>&,
                                                  Transport* transport) {
  auto boot_manager = FindBootManager();
  if (boot_manager.is_error()) {
    auto ret = SendResponse(ResponseType::kFail, "Failed to find boot manager", transport,
                            zx::error(boot_manager.status_value()));
    return zx::error(ret.status_value());
  }
  // `fastboot set_active` only cares whether the device has >1 slots. Doesn't care how many
  // exactly.
  return boot_manager->QueryCurrentConfiguration().ok() ? zx::ok("2") : zx::ok("1");
}

zx::result<std::string> Fastboot::GetVarIsUserspace(const std::vector<std::string_view>&,
                                                    Transport*) {
  return zx::ok("yes");
}

zx::result<fidl::UnownedClientEnd<fuchsia_io::Directory>> Fastboot::GetSvcRoot() {
  // If `svc_root_` is not set, use the system svc root.
  if (!svc_root_) {
    auto svc_root = component::OpenServiceRoot();
    if (svc_root.is_error()) {
      FX_LOGST(ERROR, kFastbootLogTag)
          << "Failed to connect to svc root " << svc_root.status_string();
      return zx::error(ZX_ERR_INTERNAL);
    }
    svc_root_ = *std::move(svc_root);
  }

  return zx::ok(fidl::UnownedClientEnd<fuchsia_io::Directory>(svc_root_));
}

zx::result<fidl::WireSyncClient<fuchsia_paver::Paver>> Fastboot::ConnectToPaver() {
  // Connect to the paver
  auto svc_root = GetSvcRoot();
  if (svc_root.is_error()) {
    return zx::error(svc_root.status_value());
  }
  auto paver = component::ConnectAt<fuchsia_paver::Paver>(*svc_root);
  if (!paver.is_ok()) {
    FX_LOGST(ERROR, kFastbootLogTag) << "Unable to open /svc/fuchsia.paver.Paver";
    return zx::error(paver.error_value());
  }
  return zx::ok(fidl::WireSyncClient(*std::move(paver)));
}

fuchsia_mem::wire::Buffer Fastboot::GetWireBufferFromDownload() {
  fuchsia_mem::wire::Buffer buf;
  buf.size = download_vmo_mapper_.size();
  buf.vmo = download_vmo_mapper_.Release();
  return buf;
}

zx::result<> Fastboot::WriteFirmware(fuchsia_paver::wire::Configuration config,
                                     std::string_view firmware_type, Transport* transport,
                                     fidl::WireSyncClient<fuchsia_paver::DataSink>& data_sink) {
  auto response = data_sink->WriteFirmware(config, fidl::StringView::FromExternal(firmware_type),
                                           GetWireBufferFromDownload());
  if (response.status() != ZX_OK) {
    return SendResponse(ResponseType::kFail, "Failed to invoke paver bootloader write", transport,
                        zx::error(response.status()));
  }
  const auto& result = response->result;
  if (result.is_status() && result.status() != ZX_OK) {
    return SendResponse(ResponseType::kFail, "Failed to write bootloader", transport,
                        zx::error(response->result.status()));
  }
  if (result.is_unsupported() && result.unsupported()) {
    return SendResponse(ResponseType::kFail, "Firmware type is not supported", transport);
  }
  return SendResponse(ResponseType::kOkay, "", transport);
}

zx::result<> Fastboot::WriteAsset(fuchsia_paver::wire::Configuration config,
                                  fuchsia_paver::wire::Asset asset, Transport* transport,
                                  fidl::WireSyncClient<fuchsia_paver::DataSink>& data_sink) {
  auto response = data_sink->WriteAsset(config, asset, GetWireBufferFromDownload());
  if (response.status() != ZX_OK) {
    return SendResponse(ResponseType::kFail, "Failed to invoke paver data sink write asset",
                        transport, zx::error(response.status()));
  }
  if (response->status != ZX_OK) {
    return SendResponse(ResponseType::kFail, "Failed to flash asset", transport,
                        zx::error(response->status));
  }
  return SendResponse(ResponseType::kOkay, "", transport);
}

zx::result<> Fastboot::Flash(const std::string& command, Transport* transport) {
  std::vector<std::string_view> args =
      fxl::SplitString(command, ":", fxl::kTrimWhitespace, fxl::kSplitWantNonEmpty);
  if (args.size() < 2) {
    return SendResponse(ResponseType::kFail, "Not enough arguments", transport);
  }
  FlashPartitionInfo info = GetPartitionInfo(args[1]);

  if (info.partition == "blob") {
    return FlashBlob(transport);
  }

  if (info.partition == "super") {
    return FlashSuper(transport);
  }

  if (IsSparseFormat(download_vmo_mapper_)) {
    return SendResponse(ResponseType::kFail, "Android sparse image is not supported.", transport);
  }

  auto data_sink = ConnectToDataSink(transport);
  if (data_sink.is_error()) {
    return data_sink.take_error();
  }

  if (info.partition == "bootloader") {
    // If abr suffix is not given, assume that firmware ABR is not supported and just provide a
    // A slot configuration. It will be ignored by the paver.
    fuchsia_paver::wire::Configuration config =
        info.configuration ? *info.configuration : fuchsia_paver::wire::Configuration::kA;
    std::string_view firmware_type = args.size() == 3 ? args[2] : "";
    return WriteFirmware(config, firmware_type, transport, *data_sink);
  }

  if (info.partition == "fuchsia-esp") {
    // x64 platform uses 'fuchsia-esp' for bootloader partition . We should eventually move to use
    // "bootloader"
    // For legacy `fuchsia-esp` we don't consider firmware ABR or type.
    return WriteFirmware(fuchsia_paver::wire::Configuration::kA, "", transport, *data_sink);
  }

  if (info.partition == "zircon" && info.configuration) {
    return WriteAsset(*info.configuration, fuchsia_paver::wire::Asset::kKernel, transport,
                      *data_sink);
  }

  if (info.partition == "vbmeta" && info.configuration) {
    return WriteAsset(*info.configuration, fuchsia_paver::wire::Asset::kVerifiedBootMetadata,
                      transport, *data_sink);
  }

  if (info.partition == "fvm") {
    auto response = data_sink->WriteOpaqueVolume(GetWireBufferFromDownload());
    if (response.status() != ZX_OK) {
      return SendResponse(ResponseType::kFail, "Failed to invoke paver data sink write opaque fvm",
                          transport, zx::error(response.status()));
    }
    if (response->is_error()) {
      return SendResponse(ResponseType::kFail, "Failed to flash opaque fvm", transport,
                          zx::error(response->error_value()));
    }
    return SendResponse(ResponseType::kOkay, "", transport);
  }

  if (info.partition == "gpt-meta") {
    // gpt-meta is a pseudo-partition; we don't write the contents directly to the GPT but instead
    // it provides some higher-level information about what the GPT should look like and it's up to
    // the device implementation to translate that to an actual GPT.

    if (info.configuration) {
      return SendResponse(ResponseType::kFail, "gpt-meta doesn't support slots", transport,
                          zx::error(ZX_ERR_INVALID_ARGS));
    }

    // For now we only support a single input format of a file containing the word "default", which
    // means to write the default GPT exactly as `oem init-partition-tables` would. The reason we
    // provide this alias is to simplify `ffx flash` by tying into the existing partition flash
    // mechanism rather than having to teach it about the `oem init-partition-tables` command.
    std::string_view contents(reinterpret_cast<const char*>(download_vmo_mapper_.start()),
                              download_vmo_mapper_.size());
    if (contents != kGptMetaDefault) {
      return SendResponse(ResponseType::kFail, "Invalid gpt-meta contents", transport,
                          zx::error(ZX_ERR_INVALID_ARGS));
    }

    return OemInitPartitionTables(command, transport);
  }

  return SendResponse(ResponseType::kFail, "Unsupported partition", transport);
}

zx::result<> Fastboot::Erase(const std::string& command, Transport* transport) {
  std::vector<std::string_view> args =
      fxl::SplitString(command, ":", fxl::kTrimWhitespace, fxl::kSplitWantNonEmpty);
  if (args.size() < 2) {
    return SendResponse(ResponseType::kFail, "Not enough arguments", transport);
  }
  auto partition_label = args[1];
  // We only support erasing the userdata partition.
  if (partition_label != "userdata") {
    FX_LOGST(ERROR, kFastbootLogTag) << "Ignoring request to erase partition: " << partition_label;
    return SendResponse(ResponseType::kFail, "Unknown partition", transport);
  }
  return WipeUserdata(transport);
}

zx::result<fidl::WireSyncClient<fuchsia_paver::BootManager>> Fastboot::FindBootManager() {
  auto paver = ConnectToPaver();
  if (!paver.is_ok()) {
    return zx::error(paver.status_value());
  }
  auto [client, server] = fidl::Endpoints<fuchsia_paver::BootManager>::Create();
  auto response = paver->FindBootManager(std::move(server));
  if (!response.ok()) {
    FX_LOGST(ERROR, kFastbootLogTag) << "Failed to find boot manager";
    return zx::error(response.status());
  }
  return zx::ok(fidl::WireSyncClient(std::move(client)));
}

zx::result<> Fastboot::SetActive(const std::string& command, Transport* transport) {
  std::vector<std::string_view> args =
      fxl::SplitString(command, ":", fxl::kTrimWhitespace, fxl::kSplitWantNonEmpty);
  if (args.size() < 2) {
    return SendResponse(ResponseType::kFail, "Not enough arguments", transport);
  }

  auto boot_manager = FindBootManager();
  if (boot_manager.is_error()) {
    return SendResponse(ResponseType::kFail, "Failed to find boot manager", transport,
                        zx::error(boot_manager.status_value()));
  }

  fuchsia_paver::wire::Configuration config = fuchsia_paver::wire::Configuration::kB;
  if (args[1] == "a") {
    config = fuchsia_paver::wire::Configuration::kA;
  } else if (args[1] != "b") {
    return SendResponse(ResponseType::kFail, "Invalid slot", transport);
  }

  auto response = boot_manager->SetConfigurationActive(config);
  if (response.status() != ZX_OK) {
    return SendResponse(ResponseType::kFail, "Failed to invoke paver boot manager set active",
                        transport, zx::error(response.status()));
  }
  if (response->status != ZX_OK) {
    return SendResponse(ResponseType::kFail, "Failed to set configuration active: ", transport,
                        zx::error(response->status));
  }

  return SendResponse(ResponseType::kOkay, "", transport);
}

zx::result<fidl::WireSyncClient<fuchsia_hardware_power_statecontrol::Admin>>
Fastboot::ConnectToPowerStateControl() {
  auto svc_root = GetSvcRoot();
  if (svc_root.is_error()) {
    return zx::error(svc_root.status_value());
  }
  auto admin = component::ConnectAt<fuchsia_hardware_power_statecontrol::Admin>(*svc_root);
  if (admin.is_error()) {
    return zx::error(admin.status_value());
  }
  return zx::ok(fidl::WireSyncClient(*std::move(admin)));
}

zx::result<> Fastboot::Continue(const std::string& command, Transport* transport) {
  // We cannot continue booting from userspace fastboot, so we issue a regular reboot command
  // instead so the device reboots normally.
  FX_LOGST(INFO, kFastbootLogTag) << "Userspace fastboot cannot continue, rebooting instead";
  return HandleShutdown(transport, ShutdownAction::kReboot);
}

zx::result<> Fastboot::Reboot(const std::string& command, Transport* transport) {
  return HandleShutdown(transport, ShutdownAction::kReboot);
}

zx::result<> Fastboot::RebootBootloader(const std::string& command, Transport* transport) {
  return HandleShutdown(transport, ShutdownAction::kRebootToBootloader);
}

zx::result<> Fastboot::RebootFastboot(const std::string& command, Transport* transport) {
  // Userspace fastboot runs automatically in the Fuchsia recovery image.
  return HandleShutdown(transport, ShutdownAction::kRebootToRecovery);
}

zx::result<> Fastboot::RebootRecovery(const std::string& command, Transport* transport) {
  return HandleShutdown(transport, ShutdownAction::kRebootToRecovery);
}

zx::result<> Fastboot::HandleShutdown(Transport* transport, ShutdownAction action) {
  zx::result admin = ConnectToPowerStateControl();
  if (admin.is_error()) {
    return SendResponse(ResponseType::kFail,
                        "Failed to connect to power state control service: ", transport,
                        zx::error(admin.status_value()));
  }

  // If we were writing a new blob image, we need to wait for the background sync to complete to
  // ensure there won't be any unintended data loss.
  if (blob_writer_) {
    auto status = blob_writer_->JoinSyncThread();
    blob_writer_.reset();
    if (status.is_error()) {
      return SendResponse(ResponseType::kFail, "Background sync failed", transport, status)
          .take_error();
    }
  }

  // Send an okay response early, regardless of the result. Once the system reboots or shuts down,
  // we have no chance to send the response.
  {
    zx::result response = SendResponse(ResponseType::kOkay, "", transport);
    if (response.is_error()) {
      return response;
    }
  }
  // Sleep for a short amount of time to make sure the response is sent over the transport before
  // we issue the reboot/shutdown request. This helps to ensure that the host tool receives the
  // response, otherwise it will hang waiting for a reply from the device.
  FX_LOGST(INFO, kFastbootLogTag) << "Issuing system shutdown in "
                                  << std::format("{}", kShutdownDelay) << ", action: " << action;
  std::this_thread::sleep_for(kShutdownDelay);
  // Send the shutdown request.
  fidl::Arena arena;
  auto builder = ShutdownOptions::Builder(arena);
  ShutdownReason reasons[1] = {ShutdownReason::kDeveloperRequest};
  auto vector_view = fidl::VectorView<ShutdownReason>::FromExternal(reasons);
  builder.reasons(vector_view);
  builder.action(action);
  auto response = admin->Shutdown(builder.Build());
  // We already responded to the command request, so we can't reply with any failures at this point.
  // The best we can do is log an error here.
  if (!response.ok()) {
    FX_LOGST(ERROR, kFastbootLogTag) << "Failed to invoke Shutdown: " << response;
    return zx::error(response.error().status());
  }
  if (response->is_error()) {
    zx::result<> ret = zx::error(response->error_value());
    FX_LOGST(ERROR, kFastbootLogTag) << "System shutdown failed: " << ret.status_string();
    return ret;
  }
  return zx::ok();
}

zx::result<> Fastboot::OemAddStagedBootloaderFile(const std::string& command,
                                                  Transport* transport) {
  std::vector<std::string_view> args =
      fxl::SplitString(command, " ", fxl::kTrimWhitespace, fxl::kSplitWantNonEmpty);
  if (args.size() != 3) {
    return SendResponse(ResponseType::kFail, "Invalid number of arguments", transport);
  }
  if (args[2] != sshd_host::kAuthorizedKeysBootloaderFileName) {
    return SendResponse(ResponseType::kFail, "Unsupported file: " + std::string(args[2]),
                        transport);
  }
  auto recovery = ConnectToRecoveryService();
  if (recovery.is_error()) {
    return SendResponse(ResponseType::kFail, "Failed to connect to fuchsia.fshost/Recovery",
                        transport, zx::error(recovery.status_value()));
  }

  auto response = fidl::WireCall(*recovery)->WriteDataFile(
      fidl::StringView::FromExternal(sshd_host::kAuthorizedKeyPathInData),
      download_vmo_mapper_.Release());
  if (response.status() != ZX_OK) {
    return SendResponse(ResponseType::kFail, "Failed to invoke recovery WriteDataFile", transport,
                        zx::error(response.status()));
  }
  if (response->is_error()) {
    return SendResponse(ResponseType::kFail, "Failed to write ssh key", transport,
                        zx::error(response->error_value()));
  }
  return SendResponse(ResponseType::kOkay, "", transport);
}

zx::result<fidl::WireSyncClient<fuchsia_paver::DataSink>> Fastboot::ConnectToDataSink(
    Transport* transport) {
  auto paver = ConnectToPaver();
  if (paver.is_error()) {
    return SendResponse(ResponseType::kFail, "Failed to connect to paver", transport,
                        zx::error(paver.status_value()))
        .take_error();
  }
  auto [client, server] = fidl::Endpoints<fuchsia_paver::DataSink>::Create();
  auto response = paver->FindDataSink(std::move(server));
  if (!response.ok()) {
    return SendResponse(ResponseType::kFail, "Failed to find data sink", transport,
                        zx::error(response.status()))
        .take_error();
  }
  return zx::ok(fidl::WireSyncClient{std::move(client)});
}

zx::result<fidl::WireSyncClient<fuchsia_paver::DynamicDataSink>> Fastboot::ConnectToDynamicDataSink(
    Transport* transport) {
  auto paver = ConnectToPaver();
  if (paver.is_error()) {
    return zx::error(SendResponse(ResponseType::kFail, "Failed to connect to paver", transport,
                                  zx::error(paver.status_value()))
                         .status_value());
  }
  auto [client, server] = fidl::Endpoints<fuchsia_paver::DynamicDataSink>::Create();
  auto response = paver->FindPartitionTableManager(
      fidl::ServerEnd<fuchsia_paver::DynamicDataSink>(server.TakeChannel()));
  if (!response.ok()) {
    return zx::error(SendResponse(ResponseType::kFail, "Failed to find dynamic data sink",
                                  transport, zx::error(response.status()))
                         .status_value());
  }
  return zx::ok(fidl::WireSyncClient(std::move(client)));
}

zx::result<> Fastboot::OemInitPartitionTables(const std::string& command, Transport* transport) {
  auto data_sink = ConnectToDynamicDataSink(transport);
  if (data_sink.is_error()) {
    return zx::error(data_sink.status_value());
  }
  auto response = data_sink->InitializePartitionTables();
  if (response.status() != ZX_OK) {
    return SendResponse(ResponseType::kFail, "Failed to invoke paver partition table init",
                        transport, zx::error(response.status()));
  }
  if (response->status != ZX_OK) {
    return SendResponse(ResponseType::kFail, "Failed to init partition table", transport,
                        zx::error(response->status));
  }
  return SendResponse(ResponseType::kOkay, "", transport);
}

zx::result<> Fastboot::OemInstallFromUsb(const std::string& command, Transport* transport) {
  // Command format: oem install-from-usb [source [destination]].
  // Source and/or destination can be the string "default" in order to use the default target.
  //
  // Note: source/destination aren't really usable at the moment because the path names will
  // often be too large to fit in the fastboot packet. Example paths from a NUC11:
  //   * "/dev/sys/platform/pt/PC00/bus/00:14.0/00:14.0/xhci/usb-bus/001/001/ifc-000/ums/scsi-block-device-0-0/block"
  //   * "/dev/sys/platform/pt/PC00/bus/01:00.0/01:00.0/nvme/namespace-1/block"
  // We'll need to implement some sort of substring matching to make this actually useful.
  // Probably also a way to list the current disks so the user doesn't have to magically know this
  // entire string.
  std::vector<std::string_view> args =
      fxl::SplitString(command, " ", fxl::kTrimWhitespace, fxl::kSplitWantNonEmpty);

  // Make a copy of any arg we need to pass because they must be null-terminated c-strings.
  std::string source, dest;
  if (args.size() > 2 && args[2] != "default") {
    source = args[2];
  }
  if (args.size() > 3 && args[3] != "default") {
    dest = args[3];
  }

  zx_status_t status = install_from_usb(source.empty() ? nullptr : source.c_str(),
                                        dest.empty() ? nullptr : dest.c_str());
  if (status != ZX_OK) {
    return SendResponse(ResponseType::kFail, "Failed to install from USB", transport,
                        zx::error(status));
  }
  return SendResponse(ResponseType::kOkay, "", transport);
}

zx::result<> Fastboot::OemWipePartitionTables(const std::string& command, Transport* transport) {
  auto data_sink = ConnectToDynamicDataSink(transport);
  if (data_sink.is_error()) {
    return zx::error(data_sink.status_value());
  }
  auto response = data_sink->WipePartitionTables();
  if (response.status() != ZX_OK) {
    return SendResponse(ResponseType::kFail, "Failed to invoke paver partition table wipe",
                        transport, zx::error(response.status()));
  }
  if (response->status != ZX_OK) {
    return SendResponse(ResponseType::kFail, "Failed to wipe partition table", transport,
                        zx::error(response->status));
  }
  return SendResponse(ResponseType::kOkay, "", transport);
}

zx::result<fidl::UnownedClientEnd<fuchsia_fshost::Recovery>> Fastboot::ConnectToRecoveryService() {
  if (fshost_recovery_) {
    return zx::ok(fshost_recovery_.borrow());
  }
  auto svc_root = GetSvcRoot();
  if (svc_root.is_error()) {
    return zx::error(svc_root.status_value());
  }
  auto fshost_recovery = component::ConnectAt<fuchsia_fshost::Recovery>(*svc_root);
  if (fshost_recovery.is_error()) {
    return zx::error(fshost_recovery.status_value());
  }
  fshost_recovery_ = *std::move(fshost_recovery);
  return zx::ok(fshost_recovery_.borrow());
}

zx::result<> Fastboot::OemInstallBlobImage(const std::string& command, Transport* transport) {
  auto recovery = ConnectToRecoveryService();
  if (recovery.is_error()) {
    return SendResponse(ResponseType::kFail, "Failed to connect to recovery", transport,
                        zx::error(recovery.status_value()));
  }

  // *NOTE*: InstallBlobImage requires exclusive access to the system container, and will
  // block if the system container is currently mounted. By dropping the writer state, we release
  // the mount token we got when flashing the blob volume, allowing installation to proceed.
  if (blob_writer_) {
    auto status = blob_writer_->JoinSyncThread();
    blob_writer_.reset();
    if (status.is_error()) {
      return SendResponse(ResponseType::kFail, "Background sync failed", transport, status)
          .take_error();
    }
  }

  auto response = fidl::WireCall(*recovery)->InstallBlobImage();
  if (response.status() != ZX_OK) {
    return SendResponse(ResponseType::kFail, "Recovery.InstallBlobImage Failed", transport,
                        zx::error(response.status()));
  }
  if (response->is_error()) {
    return SendResponse(ResponseType::kFail, "Failed to install blob image", transport,
                        zx::error(response->error_value()));
  }
  return SendResponse(ResponseType::kOkay, "", transport);
}

zx::result<> Fastboot::UpdateSuper(const std::string& command, Transport* transport) {
  std::vector<std::string_view> args =
      fxl::SplitString(command, ":", fxl::kTrimWhitespace, fxl::kSplitWantNonEmpty);
  // We only support update-super:super and update-super:super:wipe at the moment.
  if (args.size() < 2) {
    return SendResponse(ResponseType::kFail, "Not enough arguments", transport);
  }
  if (args.size() > 3) {
    return SendResponse(ResponseType::kFail, "Too many arguments", transport);
  }
  if (args[1] != "super") {
    return SendResponse(ResponseType::kFail, "Invalid target for update-super (must be super)",
                        transport);
  }
  if (args.size() == 3) {
    if (args[2] == "wipe") {
      return WipeUserdata(transport);
    }
    return SendResponse(ResponseType::kFail, "Invalid option for update-super:super", transport);
  }
  return SendResponse(ResponseType::kOkay, "", transport);
}

zx::result<> Fastboot::PrepareFlashBlob(Transport* transport) {
  switch (flash_blob_target_) {
    case FlashBlobTarget::kBlob:
      break;
    case FlashBlobTarget::kSuper:
      return zx::ok();  // We'll overwrite super instead, nothing to prepare.
  }
  // Ensure this chunk is in the Android sparse format.
  const std::optional<uint64_t> unsparsed_size = GetUnsparsedSize(download_vmo_mapper_);
  if (!unsparsed_size.has_value()) {
    return SendResponse(ResponseType::kFail, "blob image must be in Android sparse format.",
                        transport, zx::error(ZX_ERR_NOT_SUPPORTED))
        .take_error();
  }
  // If we already have a valid image handle from a previous chunk, ensure we have enough space for
  // this one.
  if (blob_writer_) {
    if (auto status = blob_writer_->EnsureSize(*unsparsed_size); status.is_error()) {
      return SendResponse(ResponseType::kFail, "Failed to resize image file", transport, status)
          .take_error();
    }
    return zx::ok();
  }

  auto recovery = ConnectToRecoveryService();
  auto response = fidl::WireCall(*recovery)->GetBlobImageHandle();
  if (response.status() != ZX_OK) {
    return SendResponse(ResponseType::kFail, "Recovery.GetBlobImageHandle transport error",
                        transport, zx::error(response.status()))
        .take_error();
  }
  if (response->is_error()) {
    // If we suspect the system container is corrupt or otherwise cannot be mounted, suggest a
    // possible remedial action.
    // TODO(https://fxbug.dev/458436824): Add a separate error to distinguish between suspected
    // corruption and incorrect on-disk version.
    if (response->error_value() == ZX_ERR_IO_DATA_INTEGRITY) {
      return SendResponse(ResponseType::kFail,
                          "Filesystem cannot be mounted (may be corrupt or incorrect version)."
                          " Device may require full-wipe flash or a newer system image.",
                          transport, zx::error(ZX_ERR_IO_DATA_INTEGRITY))
          .take_error();
    }
    return SendResponse(ResponseType::kFail, "Recovery.GetBlobImageHandle failed", transport,
                        response->take_error())
        .take_error();
  }

  fidl::ClientEnd<fuchsia_io::File> image_file;
  zx::eventpair mount_token;

  switch ((*response)->Which()) {
    case fuchsia_fshost::wire::RecoveryGetBlobImageHandleResponse::Tag::kUnformatted:
      // The system container is unformatted or has the wrong format, let's overwrite the super
      // partition instead.
      FX_LOGST(WARNING, kFastbootLogTag)
          << "Filesystem has wrong format, overwriting `super` instead.";
      flash_blob_target_ = FlashBlobTarget::kSuper;
      return zx::ok();
    case fuchsia_fshost::wire::RecoveryGetBlobImageHandleResponse::Tag::kMountedSystemContainer:
      image_file = std::move((*response)->mounted_system_container().image_file);
      mount_token = std::move((*response)->mounted_system_container().mount_token);
      break;
  }

  auto resize_response = fidl::WireCall(image_file)->Resize(*unsparsed_size);
  if (resize_response.status() != ZX_OK) {
    return SendResponse(ResponseType::kFail, "Transport error when resizing image file", transport,
                        zx::error(resize_response.status()))
        .take_error();
  }
  if (resize_response->is_error()) {
    return SendResponse(ResponseType::kFail, "Failed to resize image file", transport,
                        zx::error(resize_response->error_value()))
        .take_error();
  }

  zx::vmo file_vmo;
  {
    auto backing_memory =
        fidl::WireCall(image_file)
            ->GetBackingMemory(fuchsia_io::VmoFlags::kRead | fuchsia_io::VmoFlags::kWrite |
                               fuchsia_io::VmoFlags::kSharedBuffer);
    if (backing_memory.status() != ZX_OK) {
      return SendResponse(ResponseType::kFail,
                          "Transport error getting backing memory for image file", transport,
                          zx::error(backing_memory.status()))
          .take_error();
    }
    if (backing_memory->is_error()) {
      return SendResponse(ResponseType::kFail, "Failed to get backing VMO for image file",
                          transport, zx::error(backing_memory->error_value()))
          .take_error();
    }
    file_vmo = std::move(backing_memory->value()->vmo);
  }

  fzl::OwnedVmoMapper fill_buffer;
  if (zx_status_t status =
          fill_buffer.CreateAndMap(kFillBufferNumPages * zx_system_get_page_size(),
                                   "sparse-fill-buff", ZX_VM_PERM_READ | ZX_VM_PERM_WRITE);
      status != ZX_OK) {
    return SendResponse(ResponseType::kFail, "Failed to create and map fill buffer", transport,
                        zx::error(status))
        .take_error();
  }

  blob_writer_ = std::make_unique<BlobImageWriter>(std::move(image_file), std::move(mount_token),
                                                   std::move(file_vmo), std::move(fill_buffer),
                                                   *unsparsed_size);
  return zx::ok();
}

zx::result<> Fastboot::FlashBlob(Transport* transport) {
  auto result = PrepareFlashBlob(transport);
  if (result.is_error()) {
    return result.take_error();
  }
  if (flash_blob_target_ == FlashBlobTarget::kSuper) {
    return FlashSuper(transport);
  }
  ZX_ASSERT(blob_writer_ != nullptr);  // Should be initialized by PrepareFlashBlob.

  if (auto status = blob_writer_->CheckSyncError(); status.is_error()) {
    return SendResponse(ResponseType::kFail, "Background sync failed", transport, status)
        .take_error();
  }

  // Unsparse the download buffer directly into the file-backed VMO.
  auto unsparse_result = Unsparse(/*src=*/download_vmo_mapper_, /*dst=*/blob_writer_->file_vmo(),
                                  blob_writer_->fill_buffer(), &LogUnsparseError);
  if (unsparse_result.is_error()) {
    return SendResponse(ResponseType::kFail, "Failed to unsparse payload", transport,
                        unsparse_result.take_error());
  }

  blob_writer_->QueueSync();
  return SendResponse(ResponseType::kOkay, "", transport);
}

zx::result<> Fastboot::FlashSuper(Transport* transport) {
  // Make sure we aren't writing to the blob volume, since we're about to overwrite all of super.
  if (blob_writer_) {
    auto status = blob_writer_->JoinSyncThread();
    blob_writer_.reset();
    if (status.is_error()) {
      // Don't fail the whole operation, just log the error.
      FX_LOGST(ERROR, kFastbootLogTag) << "Background sync failed: " << status.status_string();
    }
  }

  const std::optional<uint64_t> unsparsed_size = GetUnsparsedSize(download_vmo_mapper_);
  if (!unsparsed_size.has_value()) {
    return SendResponse(ResponseType::kFail, "super must be in Android sparse format.", transport,
                        zx::error(ZX_ERR_NOT_SUPPORTED))
        .take_error();
  }
  // Write the sparse chunk directly to the super partition via the paver.
  auto data_sink = ConnectToDataSink(transport);
  if (data_sink.is_error()) {
    return data_sink.take_error();
  }
  auto response = data_sink->WriteSparseVolume(GetWireBufferFromDownload());
  if (response.status() != ZX_OK) {
    return SendResponse(ResponseType::kFail,
                        "Failed to invoke fuchsia.paver/DataSink.WriteSparseVolume", transport,
                        zx::error(response.status()));
  }
  if (response->is_error()) {
    return SendResponse(ResponseType::kFail, "Failed to flash super", transport,
                        zx::error(response->error_value()));
  }
  return SendResponse(ResponseType::kOkay, "", transport);
}

zx::result<> Fastboot::WipeUserdata(Transport* transport) {
  auto svc_root = GetSvcRoot();
  if (svc_root.is_error()) {
    return zx::error(svc_root.status_value());
  }
  auto fshost_admin = component::ConnectAt<fuchsia_fshost::Admin>(*svc_root);
  if (fshost_admin.is_error()) {
    return zx::error(fshost_admin.status_value());
  }
  FX_LOGST(INFO, kFastbootLogTag) << "Shredding data volume. Data will be permanently lost!";
  auto response = fidl::WireCall(*fshost_admin)->ShredDataVolume();
  if (response.status() != ZX_OK) {
    return SendResponse(ResponseType::kFail, "Failed to invoke ShredDataVolume", transport,
                        zx::error(response.status()));
  }
  // TODO(https://fxbug.dev/464027981): This command will always succeed on an unprovisioned device,
  // but will fail if we detect a valid superblock but fail to mount the filesystem. As long as we
  // rotate hardware keys, we can allow the command to succeed. In the meantime, we should consider
  // re-formatting the system container as a remedial action here on any failures.
  if (response->is_error()) {
    return SendResponse(ResponseType::kFail, "Failed to shred data volume", transport,
                        zx::error(response->error_value()));
  }
  // Since we successfully shredded the data volume, there's no need to preserve any data in the
  // system container. This means we can allow subsequent requests to flash the blob volume to
  // instead directly overwrite the super partition with the new system image.
  flash_blob_target_ = FlashBlobTarget::kSuper;
  return SendResponse(ResponseType::kOkay, "", transport);
}

Fastboot::BlobImageWriter::BlobImageWriter(fidl::ClientEnd<fuchsia_io::File> image_file,
                                           zx::eventpair mount_token, zx::vmo file_vmo,
                                           fzl::OwnedVmoMapper fill_buffer, uint64_t image_size)
    : image_file_(std::move(image_file)),
      mount_token_(std::move(mount_token)),
      file_vmo_(std::move(file_vmo)),
      fill_buffer_(std::move(fill_buffer)),
      image_size_(image_size) {
  // Start the background thread that waits for chunks to be written and syncs them in parallel.
  sync_thread_ = std::jthread([this](std::stop_token stoken) {
    while (true) {
      {
        std::unique_lock lock(mutex_);
        cv_.wait(lock, stoken, [this] { return sync_requested_; });
        const bool sync_requested = std::exchange(sync_requested_, false);
        if (!sync_requested && stoken.stop_requested()) {
          break;
        }
      }

      auto sync_response = fidl::WireCall(image_file_)->Sync();
      if (sync_response.status() != ZX_OK) {
        std::lock_guard lock(mutex_);
        sync_error_ = zx::error(sync_response.status());
        break;
      }
      if (sync_response->is_error()) {
        std::lock_guard lock(mutex_);
        sync_error_ = zx::error(sync_response->error_value());
        break;
      }
    }
  });
}

Fastboot::BlobImageWriter::~BlobImageWriter() {
  sync_thread_.request_stop();
  cv_.notify_one();
}

zx::result<> Fastboot::BlobImageWriter::EnsureSize(uint64_t new_size) {
  if (new_size <= image_size_) {
    return zx::ok();
  }
  auto resize_response = fidl::WireCall(image_file_)->Resize(new_size);
  if (resize_response.status() != ZX_OK) {
    return zx::error(resize_response.status());
  }
  if (resize_response->is_error()) {
    return zx::error(resize_response->error_value());
  }
  image_size_ = new_size;
  return zx::ok();
}

zx::result<> Fastboot::BlobImageWriter::CheckSyncError() {
  std::lock_guard lock(mutex_);
  return sync_error_ ? *sync_error_ : zx::ok();
}

zx::result<> Fastboot::BlobImageWriter::JoinSyncThread() {
  if (sync_thread_.joinable()) {
    sync_thread_.request_stop();
    sync_thread_.join();
  }
  return sync_error_ ? *sync_error_ : zx::ok();
}

void Fastboot::BlobImageWriter::QueueSync() {
  ZX_ASSERT(sync_thread_.joinable());
  {
    std::lock_guard lock(mutex_);
    sync_requested_ = true;
  }
  cv_.notify_one();
}

}  // namespace fastboot
