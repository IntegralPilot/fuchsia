// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.ldsvc/cpp/wire.h>
#include <lib/elfldltl/testing/get-test-data.h>
#include <lib/zx/channel.h>
#include <lib/zx/vmo.h>
#include <zircon/dlfcn.h>
#include <zircon/status.h>
#include <zircon/types.h>

#ifdef USE_ZXTEST
#include <zxtest/zxtest.h>
#else
#include <gtest/gtest.h>
#endif

namespace elfldltl::testing {

// The fdio open doesn't support getting an fd that will allow PROT_EXEC mmap
// usage, i.e. yield a VMO with ZX_RIGHT_EXECUTE.  Instead, use the loader
// service to look up the file in /pkg/lib. It is permissible for the VMO to not
// be found, but otherwise gtest assertions are used for all other failures.
// This should only be used inside a gtest Test(...) function.
zx::vmo TryGetTestLibVmo(std::string_view libname) {
  constexpr auto init_ldsvc = []() {
    // The dl_set_loader_service API replaces the handle used by `dlopen` et al
    // and returns the old one, so initialize by borrowing that handle while
    // leaving it intact in the system runtime.
    zx::unowned_channel channel{dl_set_loader_service(ZX_HANDLE_INVALID)};
    EXPECT_TRUE(channel->is_valid());
    zx_handle_t reset = dl_set_loader_service(channel->get());
    EXPECT_EQ(reset, ZX_HANDLE_INVALID);
    return fidl::UnownedClientEnd<fuchsia_ldsvc::Loader>{channel};
  };
  static const auto ldsvc_endpoint = init_ldsvc();

  fidl::Arena arena;
  fidl::WireResult result = fidl::WireCall(ldsvc_endpoint)->LoadObject({arena, libname});
  // Expect the FIDL call to succeed.
  EXPECT_TRUE(result.ok()) << libname << ": " << zx_status_get_string(result.status());
  // If the VMO was not found, return an empty object.
  if (result->rv == ZX_ERR_NOT_FOUND) {
    return zx::vmo(ZX_HANDLE_INVALID);
  }
  EXPECT_EQ(result->rv, ZX_OK) << libname << ": " << zx_status_get_string(result->rv);
  return std::move(result->object);
}

// Note this uses gtest assertions for all failures and so must be used inside
// a gtest TEST(...) function.
zx::vmo GetTestLibVmo(std::string_view libname) {
  auto vmo = TryGetTestLibVmo(libname);
  EXPECT_TRUE(vmo) << libname << ": not found";
  return vmo;
}

}  // namespace elfldltl::testing
