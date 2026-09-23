// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_FS_MANAGEMENT_CPP_MKFS_WITH_DEFAULT_H_
#define SRC_STORAGE_LIB_FS_MANAGEMENT_CPP_MKFS_WITH_DEFAULT_H_

#include <fidl/fuchsia.fxfs/cpp/wire.h>
#include <lib/zx/result.h>

#include "src/storage/lib/fs_management/cpp/component.h"
#include "src/storage/lib/fs_management/cpp/options.h"

namespace fs_management {

// Like Mkfs but creates a "default" volume (which will only work on multi-volume filesystems)
// and will be encrypted using `crypt_client`, if set.
// This should only be used for testing.
zx::result<> MkfsWithDefault(const char* device_path, FsComponent& component,
                             const MkfsOptions& options,
                             fidl::ClientEnd<fuchsia_fxfs::Crypt> crypt_client);

}  // namespace fs_management

#endif  // SRC_STORAGE_LIB_FS_MANAGEMENT_CPP_MKFS_WITH_DEFAULT_H_
