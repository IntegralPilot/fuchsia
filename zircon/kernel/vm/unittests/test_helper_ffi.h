// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_VM_UNITTESTS_TEST_HELPER_FFI_H_
#define ZIRCON_KERNEL_VM_UNITTESTS_TEST_HELPER_FFI_H_

#include <stdbool.h>
#include <stddef.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#include <vm/attribution.h>

#include "vm/vm_object.h"
#include "vm/vm_object_paged.h"

__BEGIN_CDECLS

zx_status_t cpp_make_committed_pager_vmo(size_t num_pages, bool trap_dirty, bool resizable,
                                         vm_page_t** out_pages, VmObjectPaged** out_vmo);

zx_status_t cpp_make_partially_committed_pager_vmo(size_t num_pages, size_t committed_pages,
                                                   bool trap_dirty, bool resizable,
                                                   bool ignore_requests, vm_page_t** out_pages,
                                                   VmObjectPaged** out_vmo);

zx_status_t cpp_supply_pager_vmo_pages(VmObjectPaged* vmo, uint64_t page_offset, uint64_t num_pages,
                                       vm_page_t** out_pages);

bool cpp_verify_continuous_attribution_bytes(VmObject* vmo, uint64_t expected_bytes);
void cpp_make_private_attribution_counts(uint64_t uncompressed, uint64_t compressed,
                                         vm::AttributionCounts* out_counts);
void cpp_change_vmo_high_priority_count(VmObjectPaged* vmo, int64_t delta);

void cpp_fill_region(uintptr_t seed, void* ptr, size_t len);
bool cpp_test_region(uintptr_t seed, void* ptr, size_t len);
uint32_t cpp_test_rand(uint32_t seed);
__END_CDECLS

#endif  // ZIRCON_KERNEL_VM_UNITTESTS_TEST_HELPER_FFI_H_
