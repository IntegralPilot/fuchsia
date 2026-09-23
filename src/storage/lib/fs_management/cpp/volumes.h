// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_FS_MANAGEMENT_CPP_VOLUMES_H_
#define SRC_STORAGE_LIB_FS_MANAGEMENT_CPP_VOLUMES_H_

#include <fidl/fuchsia.fs.startup/cpp/wire.h>
#include <fidl/fuchsia.io/cpp/markers.h>
#include <fidl/fuchsia.io/cpp/wire.h>

#include "lib/zx/channel.h"

namespace fs_management {

// Adds volume |name| to the filesystem instance.  |options.crypt| is an optional channel to a Crypt
// service, in which case the volume will be encrypted.
//
// On success, |outgoing_dir| will be passed to the filesystem and bound to the volume's outgoing
// directory.  The channel will be closed on failure.
//
// Currently this is only supported for Fxfs.
__EXPORT zx::result<> CreateVolume(fidl::UnownedClientEnd<fuchsia_io::Directory> exposed_dir,
                                   std::string_view name,
                                   fidl::ServerEnd<fuchsia_io::Directory> outgoing_dir,
                                   fuchsia_fs_startup::wire::CreateOptions create_options,
                                   fuchsia_fs_startup::wire::MountOptions options);

// Opens volume |name| in the filesystem instance.  |crypt_client| is an optional channel to
// a Crypt service instance, in which case the volume is decrypted using that service.
//
// On success, |outgoing_dir| will be passed to the filesystem and bound to the volume's outgoing
// directory.  The channel will be closed on failure.
//
// Currently this is only supported for Fxfs.
__EXPORT zx::result<> OpenVolume(fidl::UnownedClientEnd<fuchsia_io::Directory> exposed_dir,
                                 std::string_view name,
                                 fidl::ServerEnd<fuchsia_io::Directory> outgoing_dir,
                                 fuchsia_fs_startup::wire::MountOptions options);

// Checks volume |name| in the filesystem instance.  |crypt_client| is an optional channel to
// a Crypt service instance, in which case the volume is decrypted using that service.
//
// Currently this is only supported for Fxfs.
__EXPORT zx::result<> CheckVolume(fidl::UnownedClientEnd<fuchsia_io::Directory> exposed_dir,
                                  std::string_view name,
                                  fidl::ClientEnd<fuchsia_fxfs::Crypt> crypt_client = {});

__EXPORT zx::result<> RemoveVolume(fidl::UnownedClientEnd<fuchsia_io::Directory> exposed_dir,
                                   std::string_view name);

// Checks if |name| exists in the filesystem instance.
//
// Currently this is only supported for Fxfs.
__EXPORT bool HasVolume(fidl::UnownedClientEnd<fuchsia_io::Directory> exposed_dir,
                        std::string_view name);

}  // namespace fs_management

#endif  // SRC_STORAGE_LIB_FS_MANAGEMENT_CPP_VOLUMES_H_
