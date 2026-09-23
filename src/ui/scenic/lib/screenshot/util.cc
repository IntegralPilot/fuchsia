// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/screenshot/util.h"

#include <lib/syslog/cpp/macros.h>

namespace screenshot {

bool ServeScreenshot(
    fidl::ServerEnd<fuchsia_io::File> file_server, zx::vmo response_vmo, size_t screenshot_index,
    std::unordered_map<size_t,
                       std::pair<std::unique_ptr<vfs::VmoFile>, std::unique_ptr<async::WaitOnce>>>*
        served_screenshots) {
  size_t vmo_size;
  if (zx_status_t zx_status = response_vmo.get_size(&vmo_size); zx_status != ZX_OK) {
    FX_PLOGS(ERROR, zx_status) << "Unable to get VMO size";
    return false;
  }

  std::vector<uint8_t> buf;
  buf.resize(1);

  bool vmo_is_readable = (response_vmo.read(buf.data(), 0, 1) == ZX_OK);

  if (!vmo_is_readable) {
    zx::vmo readable_vmo;
    if (zx_status_t zx_status =
            GenerateReadableVmo(std::move(response_vmo), vmo_size, &readable_vmo);
        zx_status) {
      return false;
    }

    size_t readable_vmo_size;
    if (zx_status_t zx_status = readable_vmo.get_size(&readable_vmo_size); zx_status != ZX_OK) {
      FX_PLOGS(ERROR, zx_status) << "Unable to get new VMO size";
      return false;
    }
    if (readable_vmo_size != vmo_size) {
      FX_LOGS(ERROR) << "Unexpected readable vmo size";
      return false;
    }
    response_vmo = std::move(readable_vmo);
  }

  auto served_screenshot = std::make_unique<vfs::VmoFile>(std::move(response_vmo), vmo_size);
  std::unique_ptr<async::WaitOnce> channel_closed_observer =
      std::make_unique<async::WaitOnce>(file_server.channel().get(), ZX_CHANNEL_PEER_CLOSED);

  served_screenshots->emplace(screenshot_index, std::make_pair(std::move(served_screenshot),
                                                               std::move(channel_closed_observer)));

  std::function<void()> completed = [served_screenshots, screenshot_index]() mutable {
    served_screenshots->erase(screenshot_index);
  };

  if (const auto status =
          served_screenshots->at(screenshot_index)
              .second->Begin(async_get_default_dispatcher(),
                             [completed = std::move(completed)](...) { completed(); });
      status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Cannot attach observer to server end";
    return false;
  }

  if (const auto status = served_screenshots->at(screenshot_index)
                              .first->Serve(fuchsia_io::kPermReadable, std::move(file_server));
      status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Cannot serve screenshot";
    return false;
  }
  return true;
}

std::vector<uint8_t> ExtractVmoData(fzl::VmoMapper mapper, size_t size) {
  auto* ptr = reinterpret_cast<uint8_t*>(mapper.start());
  std::vector<uint8_t> block(ptr, ptr + size);
  mapper.Unmap();
  return block;
}

zx_status_t GenerateReadableVmo(zx::vmo response_vmo, size_t vmo_size, zx::vmo* readable_vmo) {
  fzl::VmoMapper response_vmo_mapper;

  if (zx_status_t status = response_vmo_mapper.Map(std::move(response_vmo), 0, 0, ZX_VM_PERM_READ);
      status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Failed to map VMO";
    return status;
  }

  auto buf = ExtractVmoData(std::move(response_vmo_mapper), vmo_size);

  fzl::VmoMapper readable_vmo_mapper;

  if (zx_status_t status = readable_vmo_mapper.CreateAndMap(
          vmo_size, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE,
          /*VmarManager*/ nullptr, readable_vmo,
          ZX_RIGHT_READ | ZX_RIGHT_WRITE | ZX_RIGHT_MAP | ZX_RIGHT_TRANSFER | ZX_RIGHT_DUPLICATE |
              ZX_RIGHT_GET_PROPERTY);
      status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Failed to create VMO";
    return status;
  }

  if (zx_status_t status = readable_vmo->write(buf.data(), 0, vmo_size); status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Failed to write VMO";
    return status;
  }

  return ZX_OK;
}

}  // namespace screenshot
