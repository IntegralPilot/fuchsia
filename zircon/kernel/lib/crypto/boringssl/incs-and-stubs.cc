// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <stddef.h>
#include <string.h>

#include <openssl/mem.h>

#include "third_party/boringssl/src/crypto/fipsmodule/sha/sha256.cc.inc"

// This file contains the .inc includes and stub definitions needed for a
// minimal, kernel-appropriate port of boringssl code.

// See //third_party/boringssl/src/crypto/mem.c and //third_party/boringssl/src/crypto/internal.h.
// This should match that code except that it assumes !OPENSSL_WINDOWS and OPENSSL_NO_ASM.
void OPENSSL_cleanse(void *ptr, size_t len) {
  if (len != 0) {
    memset(ptr, 0, len);
  }
}
