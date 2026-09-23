// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Test that we can include headers from //zircon/system/public. The mechanism
// differs for zx libraries depending on whether targeting Fuchsia or not.
// TODO(https://fxbug.dev/429377203): Remove the condition once
// "//zircon/system/public" is being added to `deps` for host.
#if defined(__Fuchsia__)
#include "zircon/types.h"
zx_handle_t my_handle = ZX_HANDLE_INVALID;
#endif

#if !defined(_ALL_SOURCE)
#error "`_ALL_SOURCE` should be defined when not using a Zircon-specific toolchain."
#endif
