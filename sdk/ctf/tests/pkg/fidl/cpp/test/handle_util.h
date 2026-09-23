// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef CTF_TESTS_PKG_FIDL_CPP_TEST_HANDLE_UTIL_H_
#define CTF_TESTS_PKG_FIDL_CPP_TEST_HANDLE_UTIL_H_

#include <lib/zx/handle.h>

namespace fidl {
namespace test {
namespace util {

zx_handle_t CreateChannel(zx_rights_t rights = ZX_RIGHT_SAME_RIGHTS);
zx_handle_t CreateEvent(zx_rights_t rights = ZX_RIGHT_SAME_RIGHTS);

}  // namespace util
}  // namespace test
}  // namespace fidl

#endif  // CTF_TESTS_PKG_FIDL_CPP_TEST_HANDLE_UTIL_H_
