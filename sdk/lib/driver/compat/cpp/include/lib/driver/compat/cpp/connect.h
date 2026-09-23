// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_COMPAT_CPP_CONNECT_H_
#define LIB_DRIVER_COMPAT_CPP_CONNECT_H_

#include <fidl/fuchsia.driver.compat/cpp/wire.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/driver/async-helpers/cpp/async_task.h>
#include <lib/driver/incoming/cpp/namespace.h>
#include <lib/fit/defer.h>

namespace compat {

using EntriesCallback = fit::callback<void(zx::result<std::vector<std::string>>)>;
// Make an async call to the directory to get all of the existing entries, and calls the callback
// with a vector of strings for each entry.
fdf::async_helpers::AsyncTask FindDirectoryEntries(fidl::ClientEnd<fuchsia_io::Directory> dir,
                                                   async_dispatcher_t* dispatcher,
                                                   EntriesCallback cb);

// Make an synchronous call to the directory to get all of the existing entries, returns
// a vector of strings for each entry.
zx::result<std::vector<std::string>> FindDirectoryEntries(
    fidl::ClientEnd<fuchsia_io::Directory> dir);

struct ParentDevice {
  std::string name;
  fidl::ClientEnd<fuchsia_driver_compat::Device> client;
};

using ConnectCallback = fit::callback<void(zx::result<std::vector<ParentDevice>>)>;
// Asynchronously connect to each of the parent devices.
fdf::async_helpers::AsyncTask ConnectToParentDevices(async_dispatcher_t* dispatcher,
                                                     const fdf::Namespace* ns, ConnectCallback cb);

// Synchronously connect to each of the parent devices.
zx::result<std::vector<ParentDevice>> ConnectToParentDevices(const fdf::Namespace* ns);

}  // namespace compat

#endif  // LIB_DRIVER_COMPAT_CPP_CONNECT_H_
