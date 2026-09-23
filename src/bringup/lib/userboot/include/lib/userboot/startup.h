// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_BRINGUP_LIB_USERBOOT_INCLUDE_LIB_USERBOOT_STARTUP_H_
#define SRC_BRINGUP_LIB_USERBOOT_INCLUDE_LIB_USERBOOT_STARTUP_H_

#include <zircon/compiler.h>
#include <zircon/types.h>

__BEGIN_CDECLS

// This transfers ownership of the channel from the kernel where the message
// full of system handles can be read.
zx_handle_t TakeBootstrapChannel(void);

__END_CDECLS

#endif  // SRC_BRINGUP_LIB_USERBOOT_INCLUDE_LIB_USERBOOT_STARTUP_H_
