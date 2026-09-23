// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_ROOT_RESOURCE_FILTER_INCLUDE_LIB_ROOT_RESOURCE_FILTER_H_
#define ZIRCON_KERNEL_LIB_ROOT_RESOURCE_FILTER_INCLUDE_LIB_ROOT_RESOURCE_FILTER_H_

#include <stddef.h>
#include <stdint.h>
#include <zircon/compiler.h>
#include <zircon/syscalls/resource.h>

__BEGIN_CDECLS

// Called by platform specific code to add a range to a specific resource type's
// deny list.  Must be called after global .ctors, heap initialization, and
// after blocking is permitted.  Once added to the deny list, resource ranges
// which intersect any of the denied ranges may not be created, even with the
// root resource.  This is primarily used to ensure that even user-mode code may
// not gain direct access to RAM, or to other kernel exclusive resources such as
// the interrupt controller or IOMMU.
//
// In the case of MMIO, automatic page rounding will be applied, as we cannot
// restrict access to only part of a page of MMIO.
void root_resource_filter_add_deny_region(uintptr_t base, size_t size, zx_rsrc_kind_t kind);

__END_CDECLS

#endif  // ZIRCON_KERNEL_LIB_ROOT_RESOURCE_FILTER_INCLUDE_LIB_ROOT_RESOURCE_FILTER_H_
