// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/compat/cpp/connect.h>

#include <string_view>

namespace compat {

namespace {

const std::string kCompatServicePath = std::string("/svc/") + fuchsia_driver_compat::Service::Name;

}

fdf::async_helpers::AsyncTask FindDirectoryEntries(fidl::ClientEnd<fuchsia_io::Directory> dir,
                                                   async_dispatcher_t* dispatcher,
                                                   EntriesCallback cb) {
  auto client = fidl::WireClient<fuchsia_io::Directory>(std::move(dir), dispatcher);

  fdf::async_helpers::AsyncTask task;
  // NOTE: It would be nicer to call Watch, but that is not supported in the component's
  // VFS implementation.
  client->ReadDirents(fuchsia_io::wire::kMaxBuf)
      .Then([cb = std::move(cb), completer = task.CreateCompleter()](
                fidl::WireUnownedResult<::fuchsia_io::Directory::ReadDirents>& result) mutable {
        // The format of the packed dirent structure, taken from io.fidl.
        struct dirent {
          // Describes the inode of the entry.
          uint64_t ino;
          // Describes the length of the dirent name in bytes.
          uint8_t size;
          // Describes the type of the entry. Aligned with the
          // POSIX d_type values. Use `DIRENT_TYPE_*` constants.
          uint8_t type;
          // Unterminated name of entry.
          char name[0];
        } __PACKED;

        if (!result.ok()) {
          cb(zx::error(result.status()));
          return;
        }

        size_t index = 0;
        auto& dirents = result->dirents;

        std::vector<std::string> names;

        while (index + sizeof(dirent) < dirents.size()) {
          auto packed_entry = reinterpret_cast<const dirent*>(&result->dirents[index]);
          size_t packed_entry_size = sizeof(dirent) + packed_entry->size;
          if (index + packed_entry_size > dirents.size()) {
            break;
          }
          names.emplace_back(packed_entry->name, packed_entry->size);
          index += packed_entry_size;
        }

        cb(zx::ok(std::move(names)));
      });
  task.SetItem(std::move(client));
  return task;
}

zx::result<std::vector<std::string>> FindDirectoryEntries(
    fidl::ClientEnd<fuchsia_io::Directory> dir) {
  auto client = fidl::WireSyncClient<fuchsia_io::Directory>(std::move(dir));
  // NOTE: It would be nicer to call Watch, but that is not supported in the component's
  // VFS implementation.
  fidl::WireResult result = client->ReadDirents(fuchsia_io::wire::kMaxBuf);
  if (!result.ok()) {
    return zx::error(result.status());
  }

  // The format of the packed dirent structure, taken from io.fidl.
  struct dirent {
    // Describes the inode of the entry.
    uint64_t ino;
    // Describes the length of the dirent name in bytes.
    uint8_t size;
    // Describes the type of the entry. Aligned with the
    // POSIX d_type values. Use `DIRENT_TYPE_*` constants.
    uint8_t type;
    // Unterminated name of entry.
    char name[0];
  } __PACKED;

  size_t index = 0;
  auto& dirents = result->dirents;

  std::vector<std::string> names;

  while (index + sizeof(dirent) < dirents.size()) {
    auto packed_entry = reinterpret_cast<const dirent*>(&result->dirents[index]);
    size_t packed_entry_size = sizeof(dirent) + packed_entry->size;
    if (index + packed_entry_size > dirents.size()) {
      break;
    }
    names.emplace_back(packed_entry->name, packed_entry->size);
    index += packed_entry_size;
  }

  return zx::ok(std::move(names));
}

fdf::async_helpers::AsyncTask ConnectToParentDevices(async_dispatcher_t* dispatcher,
                                                     const fdf::Namespace* ns, ConnectCallback cb) {
  auto result =
      ns->Open<fuchsia_io::Directory>(kCompatServicePath.c_str(), fuchsia_io::wire::kPermReadable);

  if (result.is_error()) {
    cb(result.take_error());
    return fdf::async_helpers::AsyncTask(true);
  }

  return FindDirectoryEntries(
      std::move(result.value()), dispatcher,
      [ns, cb = std::move(cb)](zx::result<std::vector<std::string>> entries) mutable {
        if (entries.is_error()) {
          cb(entries.take_error());
          return;
        }

        std::vector<ParentDevice> devices;
        for (auto& name : entries.value()) {
          if (name == ".") {
            continue;
          }
          auto result = ns->Connect<fuchsia_driver_compat::Device>(
              std::string(fuchsia_driver_compat::Service::Name)
                  .append("/")
                  .append(name)
                  .append("/device")
                  .c_str());
          if (result.is_error()) {
            cb(result.take_error());
            return;
          }

          devices.push_back(ParentDevice{
              .name = std::move(name),
              .client = std::move(result.value()),
          });
        }
        cb(zx::ok(std::move(devices)));
      });
}

zx::result<std::vector<ParentDevice>> ConnectToParentDevices(const fdf::Namespace* ns) {
  auto result =
      ns->Open<fuchsia_io::Directory>(kCompatServicePath.c_str(), fuchsia_io::wire::kPermReadable);
  if (result.is_error()) {
    return result.take_error();
  }

  zx::result<std::vector<std::string>> entries = FindDirectoryEntries(std::move(result.value()));
  if (entries.is_error()) {
    return entries.take_error();
  }

  std::vector<ParentDevice> devices;
  for (auto& name : entries.value()) {
    if (name == ".") {
      continue;
    }
    auto result =
        ns->Connect<fuchsia_driver_compat::Device>(std::string(fuchsia_driver_compat::Service::Name)
                                                       .append("/")
                                                       .append(name)
                                                       .append("/device")
                                                       .c_str());
    if (result.is_error()) {
      return result.take_error();
    }

    devices.push_back(ParentDevice{
        .name = std::move(name),
        .client = std::move(result.value()),
    });
  }
  return zx::ok(std::move(devices));
}

}  // namespace compat
