// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "lib/device-watcher/cpp/device-watcher.h"

#include <dirent.h>
#include <fidl/fuchsia.io/cpp/wire_types.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/unsafe.h>
#include <lib/fdio/watcher.h>
#include <lib/fit/defer.h>
#include <lib/zx/channel.h>
#include <lib/zx/time.h>
#include <string.h>

#include <climits>

namespace device_watcher {

namespace {

zx::result<fidl::ClientEnd<fuchsia_io::DirectoryWatcher>> Watch(
    fidl::UnownedClientEnd<fuchsia_io::Directory> dir) {
  auto [client, server] = fidl::Endpoints<fuchsia_io::DirectoryWatcher>::Create();
  const fidl::WireResult result =
      fidl::WireCall(dir)->Watch(fuchsia_io::wire::WatchMask::kRemoved, 0, std::move(server));
  if (!result.ok()) {
    return zx::error(result.status());
  }
  const fidl::WireResponse response = result.value();
  if (zx_status_t status = response.s; status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(std::move(client));
}

}  // namespace

__EXPORT
zx::result<DirWatcher> DirWatcher::Create(fidl::UnownedClientEnd<fuchsia_io::Directory> dir) {
  zx::result client = Watch(dir);
  if (client.is_error()) {
    return client.take_error();
  }
  return zx::ok(DirWatcher(std::move(client.value())));
}

__EXPORT
zx_status_t DirWatcher::Create(int dir_fd, std::unique_ptr<DirWatcher>* out_dir_watcher) {
  fdio_t* const io = fdio_unsafe_fd_to_io(dir_fd);
  const auto release = fit::defer([io]() { fdio_unsafe_release(io); });
  const zx_handle_t channel = fdio_unsafe_borrow_channel(io);
  zx::result client = Watch(fidl::UnownedClientEnd<fuchsia_io::Directory>(channel));
  if (client.is_ok()) {
    *out_dir_watcher = std::make_unique<DirWatcher>(std::move(client.value()));
  }
  return client.status_value();
}

__EXPORT
zx_status_t DirWatcher::WaitForRemoval(std::string_view filename, zx::duration timeout) {
  const zx::time deadline = zx::deadline_after(timeout);
  // Loop until we see the removal event, or wait_one fails due to timeout.
  for (;;) {
    zx_signals_t observed;
    if (zx_status_t status = client_.channel().wait_one(ZX_CHANNEL_READABLE, deadline, &observed);
        status != ZX_OK) {
      return status;
    }
    if (!(observed & ZX_CHANNEL_READABLE)) {
      return ZX_ERR_IO;
    }

    // Messages are of the form:
    //  uint8_t event
    //  uint8_t len
    //  char* name
    uint8_t buf[fuchsia_io::wire::kMaxBuf];
    uint32_t actual_len;
    if (zx_status_t status =
            client_.channel().read(0, buf, nullptr, sizeof(buf), 0, &actual_len, nullptr);
        status != ZX_OK) {
      return status;
    }
    if (static_cast<fuchsia_io::wire::WatchEvent>(buf[0]) !=
        fuchsia_io::wire::WatchEvent::kRemoved) {
      continue;
    }
    if (filename.length() == 0) {
      // Waiting on any file.
      return ZX_OK;
    }

    if (std::string_view{reinterpret_cast<char*>(&buf[2]), buf[1]} == filename) {
      return ZX_OK;
    }
  }
  return ZX_ERR_NOT_FOUND;
}

namespace {

zx::result<zx::channel> RecursiveWaitForFileHelper(const int dir_fd, std::string_view path,
                                                   zx::time deadline) {
  const size_t slash = path.find_first_of('/');
  const std::string_view target = slash == std::string::npos ? path : path.substr(0, slash);

  {
    auto watch_func = [](int dirfd, int event, const char* fn, void* cookie) {
      const std::string_view& target = *static_cast<std::string_view*>(cookie);
      if (event != WATCH_EVENT_ADD_FILE) {
        return ZX_OK;
      }
      if (std::string_view{fn} == target) {
        return ZX_ERR_STOP;
      }
      return ZX_OK;
    };
    // Can't be const.
    std::string_view name = target;
    if (zx_status_t status = fdio_watch_directory(dir_fd, watch_func, deadline.get(), &name);
        status != ZX_ERR_STOP) {
      return zx::error(status);
    }
  }

  zx::channel client, server;
  if (zx_status_t status = zx::channel::create(0, &client, &server); status != ZX_OK) {
    return zx::error(status);
  }

  fdio_t* const io = fdio_unsafe_fd_to_io(dir_fd);
  const auto release = fit::defer([io]() { fdio_unsafe_release(io); });
  const zx_handle_t channel = fdio_unsafe_borrow_channel(io);

  if (slash == std::string::npos) {
    if (zx_status_t status =
            fdio_service_connect_at(channel, std::string(target).c_str(), server.release());
        status != ZX_OK) {
      return zx::error(status);
    }
    return zx::ok(std::move(client));
  }
  if (zx_status_t status = fdio_open3_at(
          channel, std::string(target).c_str(),
          uint64_t{fuchsia_io::wire::kPermReadable | fuchsia_io::wire::Flags::kProtocolDirectory},
          server.release());
      status != ZX_OK) {
    return zx::error(status);
  }

  int subdir_fd;
  if (zx_status_t status = fdio_fd_create(client.release(), &subdir_fd); status != ZX_OK) {
    return zx::error(status);
  }
  const auto close_subdir = fit::defer([subdir_fd]() { close(subdir_fd); });
  return RecursiveWaitForFileHelper(subdir_fd, path.substr(slash + 1), deadline);
}

zx::result<zx::channel> RecursiveWaitForFile(const int dir_fd, std::string_view target,
                                             zx::duration timeout) {
  if (target.length() > PATH_MAX - 1) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  return RecursiveWaitForFileHelper(dir_fd, target, zx::deadline_after(timeout));
}

struct ParsedWatchEvent {
  size_t event_size() { return 2 + name.size(); }

  fuchsia_io::wire::WatchEvent event;
  std::string_view name;
};

zx::result<ParsedWatchEvent> ParseEvent(cpp20::span<uint8_t> buffer) {
  if (buffer.size() < 2) {
    return zx::error(ZX_ERR_BUFFER_TOO_SMALL);
  }
  uint8_t len = buffer[1];
  if (len + 2 > buffer.size()) {
    return zx::error(ZX_ERR_BUFFER_TOO_SMALL);
  }

  std::string_view name;
  if (len != 0) {
    name = std::string_view{reinterpret_cast<char*>(&buffer[2]), buffer[1]};
  }
  // Messages are of the form:
  //  uint8_t event
  //  uint8_t len
  //  char* name
  return zx::ok(ParsedWatchEvent{
      .event = fuchsia_io::wire::WatchEvent(buffer[0]),
      .name = name,
  });
}

}  // namespace

__EXPORT
zx::result<zx::channel> RecursiveWaitForFile(const int dir_fd, const char* path,
                                             zx::duration timeout) {
  std::string_view target{path};
  return RecursiveWaitForFile(dir_fd, target, timeout);
}

__EXPORT
zx::result<zx::channel> RecursiveWaitForFile(const char* path, zx::duration timeout) {
  const std::string_view target{path};
  if (const size_t first_slash = target.find_first_of('/'); first_slash != 0) {
    // Relative paths are not supported.
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }
  const size_t second_slash = target.find_first_of('/', 1);
  const size_t split_point = second_slash == std::string::npos ? 1 : second_slash + 1;
  const std::string_view directory = target.substr(0, split_point);
  const std::string_view rest = target.substr(split_point);

  zx::channel client, server;
  if (zx_status_t status = zx::channel::create(0, &client, &server); status != ZX_OK) {
    return zx::error(status);
  }

  if (zx_status_t status = fdio_open3(
          std::string(directory).c_str(),
          uint64_t{fuchsia_io::wire::kPermReadable | fuchsia_io::wire::Flags::kProtocolDirectory},
          server.release());
      status != ZX_OK) {
    return zx::error(status);
  }

  int dir_fd;
  if (zx_status_t status = fdio_fd_create(client.release(), &dir_fd); status != ZX_OK) {
    return zx::error(status);
  }
  const auto close_dir = fit::defer([dir_fd]() { close(dir_fd); });
  return RecursiveWaitForFile(dir_fd, rest, timeout);
}

zx::result<> WatchDirectoryForItems(fidl::UnownedClientEnd<fuchsia_io::Directory> dir,
                                    ItemCallback callback) {
  auto [client, server] = fidl::Endpoints<fuchsia_io::DirectoryWatcher>::Create();

  auto watch_mask = fuchsia_io::wire::WatchMask::kAdded | fuchsia_io::wire::WatchMask::kExisting;
  const fidl::WireResult result = fidl::WireCall(dir)->Watch(watch_mask, 0, std::move(server));
  if (!result.ok()) {
    return zx::error(result.status());
  }
  const fidl::WireResponse response = result.value();
  if (zx_status_t status = response.s; status != ZX_OK) {
    return zx::error(status);
  }

  // Loop until our callback returns an error.
  for (;;) {
    zx_signals_t observed;
    if (zx_status_t status =
            client.channel().wait_one(ZX_CHANNEL_READABLE, zx::time::infinite(), &observed);
        status != ZX_OK) {
      return zx::error(status);
    }
    if (!(observed & ZX_CHANNEL_READABLE)) {
      return zx::error(ZX_ERR_IO);
    }

    uint8_t buf[fuchsia_io::wire::kMaxBuf];
    uint32_t actual_len;
    if (zx_status_t status =
            client.channel().read(0, buf, nullptr, sizeof(buf), 0, &actual_len, nullptr);
        status != ZX_OK) {
      return zx::error(status);
    }
    cpp20::span<uint8_t> buf_span{buf, actual_len};
    while (buf_span.size() != 0) {
      zx::result parsed_result = ParseEvent(buf_span);
      if (parsed_result.is_error()) {
        return parsed_result.take_error();
      }
      ParsedWatchEvent& parsed = parsed_result.value();
      buf_span = buf_span.subspan(parsed.event_size());
      if (parsed.name == ".") {
        continue;
      }
      if (std::optional should_return = callback(parsed.name); should_return.has_value()) {
        return zx::ok();
      }
    }
  }
}

}  // namespace device_watcher
