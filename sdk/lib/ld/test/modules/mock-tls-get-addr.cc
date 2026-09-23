// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/elfldltl/layout.h>
#include <lib/ld/tls.h>

// This is a mock __tls_get_addr function that simply returns the GOT pointer
// that's passed in.

void* ld::abi::__tls_get_addr(const elfldltl::Elf<>::TlsGetAddrGot<>& got) {
  return const_cast<elfldltl::Elf<>::TlsGetAddrGot<>*>(&got);
}
