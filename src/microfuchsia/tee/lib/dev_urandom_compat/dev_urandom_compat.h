// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MICROFUCHSIA_TEE_LIB_DEV_URANDOM_COMPAT_DEV_URANDOM_COMPAT_H_
#define SRC_MICROFUCHSIA_TEE_LIB_DEV_URANDOM_COMPAT_DEV_URANDOM_COMPAT_H_

#include <zircon/compiler.h>
#include <zircon/types.h>

__BEGIN_CDECLS

// Registers an entry at /dev/urandom in the local namespace that emits
// random bytes supplied by zx_cprng_draw().
zx_status_t register_dev_urandom_compat(void);

__END_CDECLS

#endif  // SRC_MICROFUCHSIA_TEE_LIB_DEV_URANDOM_COMPAT_DEV_URANDOM_COMPAT_H_
