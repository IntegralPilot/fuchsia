// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_FIDL_CODEC_SYSCALL_PROP_STUBS_H_
#define SRC_LIB_FIDL_CODEC_SYSCALL_PROP_STUBS_H_

#include <zircon/syscalls/object.h>

// ZX_PROP_REGISTER_GS and ZX_PROP_REGISTER_FS which are used by zx_object_set_property are defined
// in <zircon/system/public/zircon/syscalls/object.h> but only available for amd64.
// We need these values in all the environments.
#ifndef ZX_PROP_REGISTER_GS
#define ZX_PROP_REGISTER_GS ((uint32_t)2u)
#endif

#ifndef ZX_PROP_REGISTER_FS
#define ZX_PROP_REGISTER_FS ((uint32_t)4u)
#endif

#endif  // SRC_LIB_FIDL_CODEC_SYSCALL_PROP_STUBS_H_
