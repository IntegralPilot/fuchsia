// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// DO NOT EDIT. Generated from FIDL library zx by zither, a Fuchsia platform
// tool.

#ifndef _ZX_SYSCALL_DECL
#error "<zircon/syscalls-next.h> is the public API header"
#endif

_ZX_SYSCALL_DECL(iob_allocate_id, zx_status_t, _ZX_SYSCALL_EXTERN_C, 6,
                 (handle, options, region_index, blob, blob_size, id),
                 (_ZX_SYSCALL_ANNO(use_handle("Fuchsia")) zx_handle_t handle,
                  zx_iob_allocate_id_options_t options, uint32_t region_index, const void* blob,
                  size_t blob_size, uint32_t* id))

_ZX_SYSCALL_DECL(iob_create_shared_region, zx_status_t, _ZX_SYSCALL_EXTERN_C, 3,
                 (options, size, out),
                 (uint64_t options, uint64_t size,
                  _ZX_SYSCALL_ANNO(acquire_handle("Fuchsia")) zx_handle_t* out))

_ZX_SYSCALL_DECL(pager_query_dirty_ranges, zx_status_t, _ZX_SYSCALL_EXTERN_C, 8,
                 (pager, pager_vmo, offset, length, buffer, buffer_size, actual, avail),
                 (_ZX_SYSCALL_ANNO(use_handle("Fuchsia")) zx_handle_t pager,
                  _ZX_SYSCALL_ANNO(use_handle("Fuchsia")) zx_handle_t pager_vmo, uint64_t offset,
                  uint64_t length, void* buffer, size_t buffer_size, size_t* actual, size_t* avail))

_ZX_SYSCALL_DECL(process_create_shared, zx_status_t, _ZX_SYSCALL_EXTERN_C, 6,
                 (shared_proc, options, name, name_size, proc_handle, restricted_vmar_handle),
                 (_ZX_SYSCALL_ANNO(use_handle("Fuchsia")) zx_handle_t shared_proc, uint32_t options,
                  const char* name, size_t name_size,
                  _ZX_SYSCALL_ANNO(acquire_handle("Fuchsia")) zx_handle_t* proc_handle,
                  _ZX_SYSCALL_ANNO(acquire_handle("Fuchsia")) zx_handle_t* restricted_vmar_handle))

_ZX_SYSCALL_DECL(restricted_bind_state, zx_status_t, _ZX_SYSCALL_EXTERN_C, 3,
                 (options, out, out_exception),
                 (uint32_t options, _ZX_SYSCALL_ANNO(acquire_handle("Fuchsia")) zx_handle_t* out,
                  zx_exception_report_t* out_exception))

_ZX_SYSCALL_DECL(restricted_enter, zx_status_t, _ZX_SYSCALL_EXTERN_C, 3,
                 (options, vector_table_ptr, context),
                 (uint32_t options, uintptr_t vector_table_ptr, uintptr_t context))

_ZX_SYSCALL_DECL(restricted_kick, zx_status_t, _ZX_SYSCALL_EXTERN_C, 2, (thread, options),
                 (_ZX_SYSCALL_ANNO(use_handle("Fuchsia")) zx_handle_t thread, uint32_t options))

_ZX_SYSCALL_DECL(restricted_unbind_state, zx_status_t, _ZX_SYSCALL_EXTERN_C, 1, (options),
                 (uint32_t options))

_ZX_SYSCALL_DECL(syscall_next_1, zx_status_t, _ZX_SYSCALL_EXTERN_C, 1, (arg), (int32_t arg))

_ZX_SYSCALL_DECL(system_suspend_enter, zx_status_t, _ZX_SYSCALL_EXTERN_C, 7,
                 (resource, resume_deadline, options, header, entries, num_entries, actual_entries),
                 (_ZX_SYSCALL_ANNO(use_handle("Fuchsia")) zx_handle_t resource,
                  zx_instant_boot_t resume_deadline, zx_system_suspend_option_t options,
                  zx_wake_source_report_header_t* header, zx_wake_source_report_entry_t* entries,
                  uint32_t num_entries, uint32_t* actual_entries))
