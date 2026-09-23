// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/fsl/io/device_watcher.h"

#include <lib/async/default.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/syslog/cpp/macros.h>

#include <utility>

namespace fio = fuchsia_io;

namespace fsl {

DeviceWatcher::DeviceWatcher(async_dispatcher_t* dispatcher,
                             fidl::ClientEnd<fuchsia_io::Directory> dir,
                             fidl::Endpoints<fuchsia_io::DirectoryWatcher> dir_watcher,
                             ExistsCallback exists_callback, IdleCallback idle_callback)
    : dir_watcher_(std::move(dir_watcher.client)),
      exists_callback_(std::move(exists_callback)),
      idle_callback_(std::move(idle_callback)),
      wait_(this, dir_watcher_.channel().get(), ZX_CHANNEL_READABLE | ZX_CHANNEL_PEER_CLOSED),
      weak_ptr_factory_(this) {
  if (dispatcher == nullptr) {
    dispatcher = async_get_default_dispatcher();
  }
  fidl::WireClient<fuchsia_io::Directory> client(std::move(dir), dispatcher);
  client
      ->Watch(fio::wire::WatchMask::kAdded | fio::wire::WatchMask::kExisting |
                  fio::wire::WatchMask::kIdle,
              0, std::move(dir_watcher.server))
      .Then([client = std::move(client), weak = weak_ptr_factory_.GetWeakPtr(), dispatcher](
                const fidl::WireUnownedResult<fuchsia_io::Directory::Watch>& result) mutable {
        if (!result.ok()) {
          if (!result.is_dispatcher_shutdown()) {
            FX_PLOGS(ERROR, result.status()) << "Failed to create device watcher";
          }
          return;
        }
        const fidl::WireResponse response = result.value();
        if (response.s != ZX_OK) {
          FX_PLOGS(ERROR, response.s) << "Failed to create device watcher";
          return;
        }
        fit::result dir = client.UnbindMaybeGetEndpoint();
        if (dir.is_error()) {
          FX_LOGS(ERROR) << "Failed to unbind directory client "
                         << dir.error_value().FormatDescription();
          return;
        }
        DeviceWatcher* self = weak.get();
        if (self == nullptr) {
          return;
        }
        self->dir_ = std::move(dir.value());
        zx_status_t status = self->wait_.Begin(dispatcher);
        FX_DCHECK(status == ZX_OK) << zx_status_get_string(status);
      });
}

std::unique_ptr<DeviceWatcher> DeviceWatcher::Create(const std::string& directory_path,
                                                     ExistsCallback exists_callback,
                                                     async_dispatcher_t* dispatcher) {
  return CreateWithIdleCallback(directory_path, std::move(exists_callback), [] {}, dispatcher);
}

std::unique_ptr<DeviceWatcher> DeviceWatcher::CreateWithIdleCallback(
    const std::string& directory_path, ExistsCallback exists_callback, IdleCallback idle_callback,
    async_dispatcher_t* dispatcher) {
  zx::result dir = component::OpenServiceRoot(directory_path);
  if (dir.is_error()) {
    FX_PLOGS(ERROR, dir.error_value()) << "Failed to open " << directory_path;
    return nullptr;
  }
  return CreateWithIdleCallback(std::move(dir.value()), std::move(exists_callback),
                                std::move(idle_callback), dispatcher);
}

std::unique_ptr<DeviceWatcher> DeviceWatcher::CreateWithIdleCallback(
    fidl::ClientEnd<fuchsia_io::Directory> dir, ExistsCallback exists_callback,
    IdleCallback idle_callback, async_dispatcher_t* dispatcher) {
  zx::result endpoints = fidl::CreateEndpoints<fio::DirectoryWatcher>();
  if (endpoints.is_error()) {
    return nullptr;
  }
  return std::unique_ptr<DeviceWatcher>(
      new DeviceWatcher(dispatcher, std::move(dir), std::move(endpoints.value()),
                        std::move(exists_callback), std::move(idle_callback)));
}

void DeviceWatcher::Handler(async_dispatcher_t* dispatcher, async::WaitBase* wait,
                            zx_status_t status, const zx_packet_signal* signal) {
  if (status != ZX_OK)
    return;

  if (signal->observed & ZX_CHANNEL_READABLE) {
    uint32_t size;
    uint8_t buf[fio::wire::kMaxBuf];
    zx_status_t read_status =
        dir_watcher_.channel().read(0, buf, nullptr, sizeof(buf), 0, &size, nullptr);
    FX_CHECK(read_status == ZX_OK) << zx_status_get_string(read_status);

    auto weak = weak_ptr_factory_.GetWeakPtr();
    uint8_t* msg = buf;
    while (size >= 2) {
      fio::wire::WatchEvent event = static_cast<fio::wire::WatchEvent>(*msg++);
      unsigned namelen = *msg++;
      if (size < (namelen + 2u)) {
        break;
      }
      if ((event == fio::wire::WatchEvent::kAdded) || (event == fio::wire::WatchEvent::kExisting)) {
        std::string filename(reinterpret_cast<char*>(msg), namelen);
        // "." is not a device, so ignore it.
        if (filename != ".") {
          exists_callback_(dir_, filename);
        }
      } else if (event == fio::wire::WatchEvent::kIdle) {
        idle_callback_();
        // Only call the idle callback once.  In case there is some captured
        // context, remove the function, or rather set it to an empty function,
        // in case we try to call it again.
        idle_callback_ = [] {};
      }
      // Note: Callback may have destroyed the DeviceWatcher before returning.
      if (!weak) {
        return;
      }
      msg += namelen;
      size -= namelen + 2;
    }
    wait->Begin(dispatcher);  // ignore errors
    return;
  }

  if (signal->observed & ZX_CHANNEL_PEER_CLOSED) {
    // TODO(jeffbrown): Should we tell someone about this?
    dir_watcher_.reset();
    return;
  }

  FX_CHECK(false);
}

}  // namespace fsl
