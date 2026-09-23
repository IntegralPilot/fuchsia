// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <dirent.h>
#include <fcntl.h>
#include <fidl/fuchsia.device/cpp/wire.h>
#include <fidl/fuchsia.hardware.ramdisk/cpp/wire.h>
#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/incoming/cpp/service.h>
#include <lib/device-watcher/cpp/device-watcher.h>
#include <lib/fdio/cpp/caller.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/fd.h>
#include <lib/fdio/fdio.h>
#include <lib/fdio/namespace.h>
#include <lib/fdio/watcher.h>
#include <lib/fit/defer.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zbi-format/partition.h>
#include <lib/zx/channel.h>
#include <lib/zx/job.h>
#include <lib/zx/time.h>
#include <lib/zx/vmo.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>
#include <zircon/process.h>
#include <zircon/processargs.h>
#include <zircon/status.h>
#include <zircon/syscalls.h>
#include <zircon/types.h>

#include <memory>
#include <utility>

#include <fbl/string.h>
#include <fbl/string_printf.h>
#include <fbl/unique_fd.h>
#include <ramdevice-client/ramdisk.h>

constexpr zx::duration kDeviceWaitTime = zx::sec(10);

struct ramdisk_client {
 public:
  DISALLOW_COPY_ASSIGN_AND_MOVE(ramdisk_client);

  static zx_status_t Create(fidl::ClientEnd<fuchsia_io::Directory> outgoing_directory,
                            zx::eventpair lifeline, std::unique_ptr<ramdisk_client>* out) {
    auto [svc_dir, server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
    if (zx_status_t status = fdio_open3_at(outgoing_directory.channel().get(), "svc",
                                           uint64_t{fuchsia_io::wire::kPermReadable},
                                           server.TakeChannel().release());
        status != ZX_OK)
      return status;

    zx::result ramdisk_interface = component::ConnectAt<fuchsia_hardware_ramdisk::Ramdisk>(svc_dir);
    if (ramdisk_interface.is_error())
      return ramdisk_interface.status_value();

    zx::result volume_interface = component::ConnectAt<fuchsia_storage_block::Block>(svc_dir);
    if (volume_interface.is_error())
      return volume_interface.status_value();
    fidl::ClientEnd<fuchsia_storage_block::Block> block_interface(volume_interface->TakeChannel());

    // Bind the outgoing directory to the namespace.
    static std::atomic<int> counter;
    // This deliberately binds to the top-level because there is/was watcher code that only worked
    // at the top-level of the local namespace (opening intermediate local directories is not
    // supported).
    std::string bind_path = "/ramdisk-" + std::to_string(counter++);
    fdio_ns_t* ns;
    if (zx_status_t status = fdio_ns_get_installed(&ns); status != ZX_OK)
      return status;
    if (zx_status_t status =
            fdio_ns_bind(ns, bind_path.c_str(), outgoing_directory.TakeHandle().release());
        status != ZX_OK)
      return status;
    fbl::String ram_disk_path = bind_path + "/svc/fuchsia.storage.block.Block";
    *out = std::unique_ptr<ramdisk_client>(new ramdisk_client(
        std::move(bind_path), std::move(ram_disk_path), *std::move(ramdisk_interface),
        std::move(block_interface), std::move(lifeline)));
    return ZX_OK;
  }

  zx_status_t Rebind() {
    if (!block_controller_)
      return ZX_ERR_NOT_SUPPORTED;
    const fidl::WireResult result = fidl::WireCall(block_controller_)->Rebind({});
    if (!result.ok()) {
      return result.status();
    }
    const fit::result response = result.value();
    if (response.is_error()) {
      return response.error_value();
    }
    ramdisk_interface_.reset();

    // Ramdisk paths have the form: /dev/.../ramctl/ramdisk-xxx/block.
    // To rebind successfully, first, we rebind the "ramdisk-xxx" path,
    // and then we wait for "block" to rebind.
    char ramdisk_path[PATH_MAX];

    // Wait for the "ramdisk-xxx" path to rebind.
    {
      const char* sep = strrchr(relative_path_.c_str(), '/');
      strlcpy(ramdisk_path, relative_path_.c_str(), sep - relative_path_.c_str() + 1);
      zx::result channel =
          device_watcher::RecursiveWaitForFile(dev_root_fd_.get(), ramdisk_path, kDeviceWaitTime);
      if (channel.is_error()) {
        return channel.error_value();
      }
      ramdisk_interface_.channel() = std::move(channel.value());
    }

    // Wait for the "block" path to rebind.
    strlcpy(ramdisk_path, relative_path_.c_str(), sizeof(ramdisk_path));
    zx::result channel =
        device_watcher::RecursiveWaitForFile(dev_root_fd_.get(), ramdisk_path, kDeviceWaitTime);
    if (channel.is_error()) {
      return channel.error_value();
    }
    block_interface_.channel() = std::move(channel.value());
    return ZX_OK;
  }

  zx_status_t Destroy() {
    if (!ramdisk_interface_) {
      return ZX_ERR_BAD_STATE;
    }

    if (ramdisk_controller_) {
      if (zx_status_t status = DestroyByHandle(ramdisk_controller_); status != ZX_OK) {
        return status;
      }
    }

    if (!bind_path_.empty()) {
      fdio_ns_t* ns;
      if (fdio_ns_get_installed(&ns) == ZX_OK)
        fdio_ns_unbind(ns, bind_path_.c_str());
      bind_path_.clear();
    }

    block_interface_.reset();
    return ZX_OK;
  }

  // Destroy all open handles to the ramdisk, while leaving the ramdisk itself attached.
  // After calling this method, destroying this object will have no effect.
  zx_status_t Forget() {
    if (!ramdisk_interface_) {
      return ZX_ERR_BAD_STATE;
    }

    ramdisk_interface_.reset();
    block_interface_.reset();
    return ZX_OK;
  }

  zx::result<fidl::ClientEnd<fuchsia_storage_block::Block>> Connect() const {
    if (block_controller_) {
      auto channel = device_watcher::RecursiveWaitForFile(dev_root_fd_.get(),
                                                          relative_path_.c_str(), kDeviceWaitTime);
      if (channel.is_error()) {
        return channel.take_error();
      }
      return zx::ok(fidl::ClientEnd<fuchsia_storage_block::Block>(std::move(channel).value()));
    }
    return component::Connect<fuchsia_storage_block::Block>(path());
  }

  fidl::UnownedClientEnd<fuchsia_device::Controller> controller_interface() const {
    return ramdisk_controller_;
  }

  fidl::UnownedClientEnd<fuchsia_hardware_ramdisk::Ramdisk> ramdisk_interface() const {
    return ramdisk_interface_.borrow();
  }

  fidl::UnownedClientEnd<fuchsia_storage_block::Block> block_interface() const {
    return block_interface_.borrow();
  }

  fidl::UnownedClientEnd<fuchsia_device::Controller> block_controller_interface() const {
    return block_controller_.borrow();
  }

  const fbl::String& path() const { return path_; }

  ~ramdisk_client() { Destroy(); }

 private:
  ramdisk_client(fbl::unique_fd dev_root_fd, fbl::String path, fbl::String relative_path,
                 fidl::ClientEnd<fuchsia_hardware_ramdisk::Ramdisk> ramdisk_interface,
                 fidl::ClientEnd<fuchsia_device::Controller> ramdisk_controller,
                 fidl::ClientEnd<fuchsia_storage_block::Block> block_interface,
                 fidl::ClientEnd<fuchsia_device::Controller> block_controller)
      : dev_root_fd_(std::move(dev_root_fd)),
        path_(std::move(path)),
        relative_path_(std::move(relative_path)),
        ramdisk_interface_(std::move(ramdisk_interface)),
        ramdisk_controller_(std::move(ramdisk_controller)),
        block_interface_(std::move(block_interface)),
        block_controller_(std::move(block_controller)) {}

  ramdisk_client(std::string bind_path, fbl::String path,
                 fidl::ClientEnd<fuchsia_hardware_ramdisk::Ramdisk> ramdisk_interface,
                 fidl::ClientEnd<fuchsia_storage_block::Block> block_interface,
                 zx::eventpair lifeline)
      : path_(std::move(path)),
        ramdisk_interface_(std::move(ramdisk_interface)),
        block_interface_(std::move(block_interface)),
        lifeline_(std::move(lifeline)),
        bind_path_(std::move(bind_path)) {}

  static zx_status_t DestroyByHandle(fidl::ClientEnd<fuchsia_device::Controller>& ramdisk) {
    const fidl::WireResult result = fidl::WireCall(ramdisk)->ScheduleUnbind();
    if (!result.ok()) {
      return result.status();
    }
    const fit::result response = result.value();
    if (response.is_error()) {
      return response.error_value();
    }
    return ZX_OK;
  }

  const fbl::unique_fd dev_root_fd_;
  // The fully qualified path.
  const fbl::String path_;
  // The path relative to dev_root_fd_.
  const fbl::String relative_path_;
  fidl::ClientEnd<fuchsia_hardware_ramdisk::Ramdisk> ramdisk_interface_;
  fidl::ClientEnd<fuchsia_device::Controller> ramdisk_controller_;
  fidl::ClientEnd<fuchsia_storage_block::Block> block_interface_;
  fidl::ClientEnd<fuchsia_device::Controller> block_controller_;

  // v2 only:
  zx::eventpair lifeline_;
  std::string bind_path_;
};

__EXPORT zx_handle_t ramdisk_get_block_interface(const ramdisk_client_t* client) {
  return client->block_interface().channel()->get();
}

__EXPORT zx_handle_t ramdisk_get_block_controller_interface(const ramdisk_client_t* client) {
  return client->block_controller_interface().channel()->get();
}

__EXPORT const char* ramdisk_get_path(const ramdisk_client_t* client) {
  return client->path().c_str();
}

__EXPORT zx_status_t ramdisk_sleep_after(const ramdisk_client* client, uint64_t block_count) {
  const fidl::WireResult result =
      fidl::WireCall(client->ramdisk_interface())->SleepAfter(block_count);
  if (!result.ok()) {
    return result.status();
  }
  return ZX_OK;
}

__EXPORT zx_status_t ramdisk_wake(const ramdisk_client* client) {
  const fidl::WireResult result = fidl::WireCall(client->ramdisk_interface())->Wake();
  if (!result.ok()) {
    return result.status();
  }
  return ZX_OK;
}

__EXPORT zx_status_t ramdisk_set_flags(const ramdisk_client* client, uint32_t flags) {
  const fidl::WireResult result =
      fidl::WireCall(client->ramdisk_interface())
          ->SetFlags(static_cast<fuchsia_hardware_ramdisk::wire::RamdiskFlag>(flags));
  if (!result.ok()) {
    return result.status();
  }
  return ZX_OK;
}

__EXPORT zx_status_t ramdisk_get_block_counts(const ramdisk_client* client,
                                              ramdisk_block_write_counts_t* out_counts) {
  static_assert(sizeof(ramdisk_block_write_counts_t) ==
                    sizeof(fuchsia_hardware_ramdisk::wire::BlockWriteCounts),
                "Cannot convert between C library / FIDL block counts");

  const fidl::WireResult result = fidl::WireCall(client->ramdisk_interface())->GetBlockCounts();
  if (!result.ok()) {
    return result.status();
  }
  const fidl::WireResponse response = result.value();
  memcpy(out_counts, &response.counts, sizeof(ramdisk_block_write_counts_t));
  return ZX_OK;
}

__EXPORT zx_status_t ramdisk_rebind(ramdisk_client_t* client) { return client->Rebind(); }

__EXPORT zx_status_t ramdisk_destroy(ramdisk_client* client) {
  zx_status_t status = client->Destroy();
  delete client;
  return status;
}

__EXPORT zx_status_t ramdisk_forget(ramdisk_client* client) {
  zx_status_t status = client->Forget();
  delete client;
  return status;
}

__EXPORT zx_status_t ramdisk_create_with_options(const ramdisk_options* options,
                                                 ramdisk_client_t** out) {
  zx::vmo vmo(options->vmo);
  fbl::unique_fd dir(
      options->svc_root_fd > 0
          ? (openat(options->svc_root_fd, "fuchsia.hardware.ramdisk.Service", O_RDONLY))
          : open("/svc/fuchsia.hardware.ramdisk.Service", O_RDONLY));
  if (!dir)
    return ZX_ERR_NOT_FOUND;
  std::string instance_name;
  zx_status_t status = fdio_watch_directory(
      dir.get(),
      [](int, int event, const char* name, void* cookie) {
        if (event == WATCH_EVENT_ADD_FILE && strcmp(name, ".") != 0) {
          *reinterpret_cast<std::string*>(cookie) = name;
          return ZX_ERR_STOP;
        }
        return ZX_OK;
      },
      ZX_TIME_INFINITE, &instance_name);
  if (status != ZX_ERR_STOP)
    return status == ZX_OK ? ZX_ERR_NOT_FOUND : status;
  zx::result<fuchsia_hardware_ramdisk::Service::ServiceClient> service;
  if (options->svc_root_fd > 0) {
    fdio_cpp::UnownedFdioCaller caller(options->svc_root_fd);
    service = component::OpenServiceAt<fuchsia_hardware_ramdisk::Service>(caller.directory(),
                                                                          instance_name);
  } else {
    service = component::OpenService<fuchsia_hardware_ramdisk::Service>(instance_name);
  }
  if (service.is_error())
    return service.status_value();
  zx::result controller = service->connect_controller();
  if (controller.is_error())
    return controller.status_value();

  fidl::Arena arena;
  fuchsia_hardware_ramdisk::wire::Guid guid;
  auto fidl_options = fuchsia_hardware_ramdisk::wire::Options::Builder(arena);
  if (options->block_size > 0)
    fidl_options.block_size(options->block_size);
  if (options->block_count > 0)
    fidl_options.block_count(options->block_count);
  if (options->type_guid) {
    memcpy(&guid.value.data_, options->type_guid, 16);
    fidl_options.type_guid(guid);
    ZX_ASSERT(fidl_options.has_type_guid());
  }
  if (vmo.is_valid())
    fidl_options.vmo(std::move(vmo));
  fidl_options.publish(true);
  const fidl::WireResult result = fidl::WireCall(*controller)->Create(fidl_options.Build());
  if (!result.ok())
    return result.status();
  fit::result response = result.value();
  if (response.is_error())
    return response.error_value();
  std::unique_ptr<ramdisk_client> client;
  if (zx_status_t status = ramdisk_client::Create(std::move(response->outgoing),
                                                  std::move(response->lifeline), &client);
      status != ZX_OK) {
    return status;
  }
  *out = client.release();
  return ZX_OK;
}

namespace ramdevice_client {

zx::result<Ramdisk> Ramdisk::Create(int block_size, uint64_t block_count,
                                    std::optional<int> svc_root_fd,
                                    const Ramdisk::Options& options) {
  ramdisk_client_t* client;
  ramdisk_options_t ramdisk_options{
      .block_size = static_cast<uint32_t>(block_size),
      .block_count = block_count,
      .type_guid = options.type_guid ? options.type_guid->data() : nullptr,
      .svc_root_fd = svc_root_fd ? *svc_root_fd : -1,
  };
  if (zx::result<> result = zx::make_result(ramdisk_create_with_options(&ramdisk_options, &client));
      result.is_error()) {
    FX_LOGS(ERROR) << "Could not create ramdisk for test: " << result.status_string();
    return result.take_error();
  }
  return zx::ok(Ramdisk(client));
}

zx::result<Ramdisk> Ramdisk::CreateWithVmo(zx::vmo vmo, uint64_t block_size,
                                           std::optional<int> svc_root_fd,
                                           const Ramdisk::Options& options) {
  ramdisk_client_t* client;
  ramdisk_options_t ramdisk_options{
      .block_size = static_cast<uint32_t>(block_size),
      .type_guid = options.type_guid ? options.type_guid->data() : nullptr,
      .vmo = vmo.release(),
      .svc_root_fd = svc_root_fd ? *svc_root_fd : -1,
  };
  if (zx::result result = zx::make_result(ramdisk_create_with_options(&ramdisk_options, &client));
      result.is_error()) {
    FX_LOGS(ERROR) << "Could not create ramdisk for test: " << result.status_string();
    return result.take_error();
  }
  return zx::ok(Ramdisk(client));
}

zx::result<fidl::ClientEnd<fuchsia_storage_block::Block>> Ramdisk::ConnectBlock() const {
  return client_->Connect();
}

}  // namespace ramdevice_client
