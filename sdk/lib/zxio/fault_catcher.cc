// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zxio/fault_catcher.h>
#include <string.h>
#include <zircon/compiler.h>

bool zxio_default_maybe_faultable_copy(unsigned char* dest, const unsigned char* src, size_t count,
                                       bool ret_dest) {
  memcpy(dest, src, count);
  return true;
}

__WEAK_ALIAS("zxio_default_maybe_faultable_copy")
bool zxio_maybe_faultable_copy(unsigned char* dest, const unsigned char* src, size_t count,
                               bool ret_dest);

__WEAK
bool zxio_fault_catching_disabled() {
  // By default, zxio does not support catching faults.
  //
  // See |zxio_fault_catching_disabled|'s and |zxio_maybe_faultable_copy|'s
  // documentation for more details.
  return true;
}
