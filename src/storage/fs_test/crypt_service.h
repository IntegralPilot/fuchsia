// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_FS_TEST_CRYPT_SERVICE_H_
#define SRC_STORAGE_FS_TEST_CRYPT_SERVICE_H_

#include <fidl/fuchsia.fxfs/cpp/wire.h>
#include <lib/fidl/cpp/wire/client.h>
#include <lib/zx/result.h>

namespace fs_test {

// Initialize the crypt service in this component's namespace with random keys, and return a handle
// to the service. Subsequent calls to this function will return a new connection to the same
// service instance.
//
// To use this function, the fxfs crypt service must be included in the package, and an appropriate
// shard must be included in the component that wants to use this. See existing usages for examples.
//
// *WARNING*: This function is **not** thread safe!
zx::result<fidl::ClientEnd<fuchsia_fxfs::Crypt>> InitializeCryptService();

}  // namespace fs_test

#endif  // SRC_STORAGE_FS_TEST_CRYPT_SERVICE_H_
