// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/ld/tls.h>

#include "../threads/thread-storage.h"
#include "ld-abi.h"

namespace LIBC_NAMESPACE_DECL {

elfldltl::TlsLayout<> ThreadStorage::GetTlsLayout() { return _ld_abi.static_tls_layout; }

void ThreadStorage::InitializeTls(std::span<std::byte> thread_block, size_t tp_offset) {
  ld::TlsInitialExecDataInit(_ld_abi, thread_block, tp_offset, true);
}

}  // namespace LIBC_NAMESPACE_DECL
