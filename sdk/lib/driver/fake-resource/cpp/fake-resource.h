// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_FAKE_RESOURCE_CPP_FAKE_RESOURCE_H_
#define LIB_DRIVER_FAKE_RESOURCE_CPP_FAKE_RESOURCE_H_

#include <zircon/syscalls/resource.h>
#include <zircon/types.h>

__BEGIN_CDECLS

zx_status_t fake_resource_create(zx_rsrc_kind_t kind, zx_handle_t *out);

__END_CDECLS

#endif  // LIB_DRIVER_FAKE_RESOURCE_CPP_FAKE_RESOURCE_H_
