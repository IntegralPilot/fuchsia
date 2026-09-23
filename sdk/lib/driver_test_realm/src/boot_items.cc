// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/component/incoming/cpp/protocol.h>
#include <lib/ddk/platform-defs.h>
#include <lib/driver_test_realm/src/boot_items.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zbi-format/board.h>
#include <lib/zbi-format/zbi.h>
#include <zircon/rights.h>

#include <ddk/metadata/test.h>

namespace driver_test_realm {

namespace {
// This board driver knows how to interpret the metadata for which devices to
// spawn.
const zbi_platform_id_t kPlatformId = []() {
  zbi_platform_id_t plat_id = {};
  plat_id.vid = PDEV_VID_TEST;
  plat_id.pid = PDEV_PID_PBUS_TEST;
  strcpy(plat_id.board_name, "driver-integration-test");
  return plat_id;
}();

#define BOARD_REVISION_TEST 42

const zbi_board_info_t kBoardInfo = []() {
  zbi_board_info_t board_info = {};
  board_info.revision = BOARD_REVISION_TEST;
  return board_info;
}();

// This function is responsible for serializing driver data. It must be kept
// updated with the function that deserialized the data. This function
// is TestBoard::FetchAndDeserialize.
zx_status_t GetBootItem(const std::vector<board_test::DeviceEntry>& entries, uint32_t type,
                        const std::string& board_name, const std::optional<uint32_t>& vid,
                        const std::optional<uint32_t>& pid, const zx::vmo& devicetree,
                        uint32_t extra, zx::vmo* out, uint32_t* length) {
  zx::vmo vmo;
  switch (type) {
    case ZBI_TYPE_PLATFORM_ID: {
      zbi_platform_id_t platform_id = kPlatformId;
      if (!board_name.empty()) {
        strncpy(platform_id.board_name, board_name.c_str(), ZBI_BOARD_NAME_LEN - 1);
      }
      if (vid.has_value()) {
        platform_id.vid = vid.value();
      }
      if (pid.has_value()) {
        platform_id.pid = pid.value();
      }
      zx_status_t status = zx::vmo::create(sizeof(kPlatformId), 0, &vmo);
      if (status != ZX_OK) {
        return status;
      }
      status = vmo.write(&platform_id, 0, sizeof(kPlatformId));
      if (status != ZX_OK) {
        return status;
      }
      *length = sizeof(kPlatformId);
      break;
    }
    case ZBI_TYPE_DRV_BOARD_INFO: {
      zx_status_t status = zx::vmo::create(sizeof(kBoardInfo), 0, &vmo);
      if (status != ZX_OK) {
        return status;
      }
      status = vmo.write(&kBoardInfo, 0, sizeof(kBoardInfo));
      if (status != ZX_OK) {
        return status;
      }
      *length = sizeof(kBoardInfo);
      break;
    }
    case ZBI_TYPE_DRV_BOARD_PRIVATE: {
      size_t list_size = sizeof(board_test::DeviceList);
      size_t entry_size = entries.size() * sizeof(board_test::DeviceEntry);

      size_t metadata_size = 0;
      for (const board_test::DeviceEntry& entry : entries) {
        metadata_size += entry.metadata_size;
      }

      zx_status_t status = zx::vmo::create(list_size + entry_size + metadata_size, 0, &vmo);
      if (status != ZX_OK) {
        return status;
      }

      // Write DeviceList to vmo.
      board_test::DeviceList list{.count = entries.size()};
      status = vmo.write(&list, 0, sizeof(list));
      if (status != ZX_OK) {
        return status;
      }

      // Write DeviceEntries to vmo.
      status = vmo.write(entries.data(), list_size, entry_size);
      if (status != ZX_OK) {
        return status;
      }

      // Write Metadata to vmo.
      size_t write_offset = list_size + entry_size;
      for (const board_test::DeviceEntry& entry : entries) {
        status = vmo.write(entry.metadata, write_offset, entry.metadata_size);
        if (status != ZX_OK) {
          return status;
        }
        write_offset += entry.metadata_size;
      }

      *length = static_cast<uint32_t>(list_size + entry_size + metadata_size);
      break;
    }
    case ZBI_TYPE_DEVICETREE: {
      zx_status_t status = devicetree.duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo);
      if (status != ZX_OK) {
        return status;
      }
      uint64_t length64;
      status = devicetree.get_size(&length64);
      if (status != ZX_OK) {
        return status;
      }
      *length = static_cast<uint32_t>(length64);
      break;
    }
    default:
      return ZX_ERR_NOT_FOUND;
  }
  *out = std::move(vmo);
  return ZX_OK;
}
}  // namespace

void BootItems::SetBoardName(std::string_view board_name) { board_name_ = std::string(board_name); }

void BootItems::SetDeviceTree(zx::vmo devicetree) { devicetree_ = std::move(devicetree); }

void BootItems::SetVid(uint32_t vid) { vid_ = vid; }

void BootItems::SetPid(uint32_t pid) { pid_ = pid; }

zx::result<> BootItems::Serve(async_dispatcher_t* dispatcher,
                              fidl::ServerEnd<fuchsia_boot::Items> server_end,
                              bool tunnel_to_incoming) {
  if (tunnel_to_incoming) {
    return component::Connect<fuchsia_boot::Items>(std::move(server_end));
  }

  bindings_.AddBinding(dispatcher, std::move(server_end), this, fidl::kIgnoreBindingClosure);
  return zx::ok();
}

void BootItems::Get(GetRequestView request, GetCompleter::Sync& completer) {
  zx::vmo vmo;
  uint32_t length = 0;
  std::vector<board_test::DeviceEntry> entries = {};
  zx_status_t status = GetBootItem(entries, request->type, board_name_, vid_, pid_, devicetree_,
                                   request->extra, &vmo, &length);
  if (status != ZX_OK) {
    FX_LOG_KV(WARNING, "Failed to get boot items", FX_KV("status", status));
  }
  completer.Reply(std::move(vmo), length);
}

void BootItems::Get2(Get2RequestView request, Get2Completer::Sync& completer) {
  std::vector<board_test::DeviceEntry> entries = {};
  zx::vmo vmo;
  uint32_t length = 0;
  uint32_t extra = 0;
  zx_status_t status = GetBootItem(entries, request->type, board_name_, vid_, pid_, devicetree_,
                                   extra, &vmo, &length);
  if (status != ZX_OK) {
    FX_LOG_KV(WARNING, "Failed to get boot items", FX_KV("status", status));
    completer.Reply(zx::error(status));
    return;
  }
  std::vector<fuchsia_boot::wire::RetrievedItems> result;
  fuchsia_boot::wire::RetrievedItems items = {
      .payload = std::move(vmo), .length = length, .extra = extra};
  result.emplace_back(std::move(items));
  completer.ReplySuccess(
      fidl::VectorView<fuchsia_boot::wire::RetrievedItems>::FromExternal(result));
}

void BootItems::GetBootloaderFile(GetBootloaderFileRequestView request,
                                  GetBootloaderFileCompleter::Sync& completer) {
  completer.Reply(zx::vmo());
}

}  // namespace driver_test_realm
