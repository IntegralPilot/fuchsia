// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DEVICE_WATCHER_CPP_DEVICE_WATCHER_H_
#define LIB_DEVICE_WATCHER_CPP_DEVICE_WATCHER_H_

#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/fit/function.h>
#include <lib/zx/channel.h>
#include <lib/zx/result.h>

#include <memory>
#include <string_view>

namespace device_watcher {

// Waits for the relative |path| starting in the directory represented by |dir_fd| to appear,
// and opens it.
// This method does not take ownership of |dir_fd|.
//
// TODO(https://fxbug.dev/42068369): Remove `timeout`.
zx::result<zx::channel> RecursiveWaitForFile(int dir_fd, const char* path,
                                             zx::duration timeout = zx::duration::infinite());

// Waits for the absolute |path| to appear, and opens it.
// NOTE: This only works for absolute paths,
// otherwise it will return ZX_ERR_NOT_SUPPORTED.
//
// TODO(https://fxbug.dev/42068369): Remove `timeout`.
zx::result<zx::channel> RecursiveWaitForFile(const char* path,
                                             zx::duration timeout = zx::duration::infinite());

using ItemCallback = fit::function<std::optional<std::monostate>(std::string_view)>;

// Call the callback for each item in the directory, and wait for new items.
//
// This function will not call the callback for the '.' file in a directory.
//
// If the callback returns a status other than `ZX_OK`, watching stops.
// If the callback returns std::nullopt the watching will continue, otherwise the
// watching will stop and zx::ok() will be returned to the caller.
zx::result<> WatchDirectoryForItems(fidl::UnownedClientEnd<fuchsia_io::Directory> dir,
                                    ItemCallback callback);

// Call the callback for each item in the directory, and wait for new items.
//
// This function will not call the callback for the '.' file in a directory.
//
// If the callback returns a status other than `ZX_OK`, watching stops.
// If the callback returns std::nullopt the watching will continue, otherwise the
// watching will stop and zx::ok() will be returned to the caller.
inline zx::result<> WatchDirectoryForItems(const fidl::ClientEnd<fuchsia_io::Directory>& dir,
                                           ItemCallback callback) {
  return WatchDirectoryForItems(fidl::UnownedClientEnd<fuchsia_io::Directory>(dir),
                                std::move(callback));
}

// A templated version of this function.
// If the callback returns std::nullopt the watching will continue, otherwise the
// watching will stop and the return value will be returned to the caller.
template <typename T>
zx::result<T> WatchDirectoryForItems(fidl::UnownedClientEnd<fuchsia_io::Directory> dir,
                                     fit::function<std::optional<T>(std::string_view)> callback) {
  std::optional<T> return_value;
  zx::result result =
      WatchDirectoryForItems(dir, [&](std::string_view name) -> std::optional<std::monostate> {
        std::optional callback_result = callback(name);
        if (!callback_result.has_value()) {
          return std::nullopt;
        }
        return_value = std::move(callback_result);
        return std::monostate();
      });

  if (result.is_error()) {
    return result.take_error();
  }
  if (!return_value.has_value()) {
    ZX_PANIC(
        "Bad state: Watching the directory returned successfully but the return value wasn't set");
  }
  return zx::ok(std::move(return_value.value()));
}

// A templated version of this function.
// If the callback returns std::nullopt the watching will continue, otherwise the
// watching will stop and the return value will be returned to the caller.
template <typename T>
zx::result<T> WatchDirectoryForItems(const fidl::ClientEnd<fuchsia_io::Directory>& dir,
                                     fit::function<std::optional<T>(std::string_view)> callback) {
  return WatchDirectoryForItems(fidl::UnownedClientEnd<fuchsia_io::Directory>(dir),
                                std::move(callback));
}

// DirWatcher can be used to detect when a file has been removed from the filesystem.
//
// Example usage:
//
//   std::unique_ptr<DirWatcher> watcher;
//   zx_status_t status = DirWatcher::Create(dir_fd, &watcher);
//   ...
//   // Trigger removal of file here.
//   ...
//   status = watcher->WaitForRemoval(filename, deadline);
class DirWatcher {
 public:
  // Creates a new |DirWatcher| instance to watch the directory represented by |dir_fd|.
  // This method does not take ownership of |dir_fd|.
  static zx_status_t Create(int dir_fd, std::unique_ptr<DirWatcher>* out_dir_watcher);

  // Creates a new |DirWatcher| instance to watch the directory |dir| is connected to..
  static zx::result<DirWatcher> Create(fidl::UnownedClientEnd<fuchsia_io::Directory> dir);

  // Users should call Create instead. This is public for make_unique.
  explicit DirWatcher(fidl::ClientEnd<fuchsia_io::DirectoryWatcher> client)
      : client_(std::move(client)) {}

  // Returns ZX_OK if |filename| is removed from the directory before the given timeout elapses.
  // If no filename is specified, this will wait for any file in the directory to be removed.
  //
  // TODO(https://fxbug.dev/42068369): Remove `timeout`.
  zx_status_t WaitForRemoval(std::string_view filename, zx::duration timeout);

 private:
  // A channel opened by a call to fuchsia.io.Directory.Watch, from which watch
  // events can be read.
  fidl::ClientEnd<fuchsia_io::DirectoryWatcher> client_;
};

}  // namespace device_watcher

#endif  // LIB_DEVICE_WATCHER_CPP_DEVICE_WATCHER_H_
