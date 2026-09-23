// Copyright 2019 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_USERABI_INCLUDE_LIB_USERABI_USERBOOT_H_
#define ZIRCON_KERNEL_LIB_USERABI_INCLUDE_LIB_USERABI_USERBOOT_H_

#include <zircon/syscalls/resource.h>

#include <object/handle.h>

HandleOwner get_resource_handle(zx_rsrc_kind_t kind);

#ifdef _KERNEL

#include <vm/handoff-end.h>

// Called at the end of the boot process in the main kernel initialization sequence.
void userboot_init(HandoffEnd handoff_end);

#endif  // _KERNEL

#endif  // ZIRCON_KERNEL_LIB_USERABI_INCLUDE_LIB_USERABI_USERBOOT_H_
