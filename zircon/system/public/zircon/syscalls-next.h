// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_SYSCALLS_NEXT_H_
#define ZIRCON_SYSCALLS_NEXT_H_

#include <stdint.h>
#include <zircon/availability.h>
#include <zircon/compiler.h>
#include <zircon/syscalls/debug.h>
#include <zircon/syscalls/exception.h>
#include <zircon/syscalls/iob.h>
#include <zircon/types.h>

#ifdef __Fuchsia__

// Only allow `syscalls-next.h` when the target API level is `HEAD` or
// `PLATFORM`.
#if FUCHSIA_API_LEVEL_LESS_THAN(HEAD)

// TODO(https://fxbug.dev/415034348): Remove this opt-out mechanism.
#ifndef FUCHSIA_UNSUPPORTED_ALLOW_SYSCALLS_NEXT_ON_INCOMPATIBLE_BUILDS
#error zircon/syscalls-next.h may not be included when targeting NEXT or a stable API level.
#endif
#endif

#else  // ifdef __Fuchsia__

// Sometimes host libraries indirectly include the syscall interface, but they
// don't have a defined target API level, so we can't check that they're
// targeting HEAD or PLATFORM.
//
// TODO(https://fxbug.dev/415033686): Remove this opt-out mechanism.
#ifndef FUCHSIA_UNSUPPORTED_ALLOW_SYSCALLS_NEXT_ON_HOST
#error zircon/syscalls-next.h may only be included in on-target code.
#endif

#endif  // ifdef __Fuchsia__

// ====== Pager writeback support ====== //

// Range type used by the zx_pager_query_dirty_ranges() syscall.
typedef struct zx_vmo_dirty_range {
  // Represents the range [offset, offset + length).
  uint64_t offset;
  uint64_t length;
  // Any options applicable to the range.
  // ZX_VMO_DIRTY_RANGE_IS_ZERO indicates that the range contains all zeros.
  uint64_t options;
} zx_vmo_dirty_range_t;

// options flags for zx_vmo_dirty_range_t
#define ZX_VMO_DIRTY_RANGE_IS_ZERO ((uint64_t)1u)

// ====== End of pager writeback support ====== //

// ====== Restricted mode support ====== //
// Structures used for the experimental restricted mode syscalls.
// Declared here in the next syscall header since it is not published
// in the SDK.

typedef uint64_t zx_restricted_reason_t;

// Reason codes provided to normal mode when a restricted process traps
// back to normal mode.
// clang-format off
#define ZX_RESTRICTED_REASON_SYSCALL        ((zx_restricted_reason_t)0)
#define ZX_RESTRICTED_REASON_EXCEPTION      ((zx_restricted_reason_t)1)
#define ZX_RESTRICTED_REASON_KICK           ((zx_restricted_reason_t)2)
#define ZX_RESTRICTED_REASON_EXCEPTION_LOST ((zx_restricted_reason_t)3)
// clang-format on

// Structure to read and write restricted mode state
//
// When exiting restricted mode for certain reasons, additional information
// may be provided by zircon. However, regardless of the reason code this
// will always be the first structure inside the restricted mode state VMO.
#if __aarch64__
typedef struct zx_restricted_state {
  uint64_t x[31];
  uint64_t sp;
  uint64_t pc;
  uint64_t tpidr_el0;
  // Contains the user-controllable upper 4-bits (NZCV) for aarch32/64.
  // If M[4] is set, aarch32 bits are accessible: T, Q, IT, and GE.
  uint32_t cpsr;
  uint8_t padding1[4];
} zx_restricted_state_t;
#elif __x86_64__
typedef struct zx_restricted_state {
  // User space active registers
  uint64_t rdi, rsi, rbp, rbx, rdx, rcx, rax, rsp;
  uint64_t r8, r9, r10, r11, r12, r13, r14, r15;
  uint64_t ip, flags;

  uint64_t fs_base, gs_base;
} zx_restricted_state_t;
#elif __riscv
typedef zx_riscv64_thread_state_general_regs_t zx_restricted_state_t;
#else
#error what architecture?
#endif

// Structure populated by zircon when exiting restricted mode with the
// reason code `ZX_RESTRICTED_REASON_SYSCALL`.
typedef struct zx_restricted_syscall {
  // Must be first.
  zx_restricted_state_t state;
} zx_restricted_syscall_t;

// Structure populated by zircon when exiting restricted mode with the
// reason code `ZX_RESTRICTED_REASON_EXCEPTION`.
typedef struct zx_restricted_exception {
  // Must be first.
  zx_restricted_state_t state;
  zx_exception_report_t exception;
} zx_restricted_exception_t;

// ====== End of restricted mode support ====== //

// ====== Wake vector support ====== //

#define ZX_INTERRUPT_WAKE_VECTOR ((uint32_t)0x20)

// ====== End wake vector support ====== //

// ====== Upcoming IOB support ====== //

// Represents an IOBuffer region of "ID allocator" discipline, used to map sized
// data blobs to sequentially-allocated numeric IDs.
//
// Suppose there are N mapped blobs. The memory is laid out as follows, with
// copies of the blobs growing down and their corresponding bookkeeping indices
// growing up:
// --------------------------------
//   next available blob ID (4 bytes)
//   blob head offset (4 bytes)
//   ----------------------------
//   blob 0 size (4 bytes)       } <-- bookkeeping index
//   blob 0 offset (4 bytes)     }
//   ...
//   blob N-1 size (4 bytes)
//   blob N-1 offset (4 bytes)
//   ----------------------------
//   zero-initialized memory      <-- remaining bytes available
//   ---------------------------- <-- blob head offset
//   blob N-1
//   ...
//   blob 0
// --------------------------------
//
#define ZX_IOB_DISCIPLINE_TYPE_ID_ALLOCATOR ((zx_iob_discipline_type_t)(1u))
#define ZX_IOB_DISCIPLINE_TYPE_MEDIATED_WRITE_RING_BUFFER ((zx_iob_discipline_type_t)(2u))

// Allocation options for `zx_iob_allocate_id()`.
typedef uint32_t zx_iob_allocate_id_options_t;

// New shared region type.
#define ZX_IOB_REGION_TYPE_SHARED ((zx_iob_region_type_t)(1u))

typedef struct zx_iob_region_shared {
  uint32_t options;
  zx_handle_t shared_region;
  uint64_t padding[3];
} zx_iob_region_shared_t;

static_assert(sizeof(zx_iob_region_shared_t) == 8 * 4);

// Ring buffer discipline.
typedef struct zx_iob_discipline_mediated_write_ring_buffer {
  uint64_t tag;
  uint64_t reserved[7];
} zx_iob_discipline_mediated_write_ring_buffer_t;

// Signals for IO Buffer shared regions.
#define ZX_IOB_SHARED_REGION_UPDATED __ZX_OBJECT_SIGNALED

// ====== End of upcoming IOB support ====== //

// ====== Start of wake vector reporting support //
//
// Support for reports generated by zx_system_suspend_enter.
//

// ZX_SYSTEM_SUSPEND_OPTION_DISCARD is an option which may be passed to
// zx_system_suspend_enter which will cause the syscall to discard any wake
// sources report entries for wake sources which have been both triggered and
// acknowledged, and are only waiting to be reported.  This operation takes
// place before any attempt to suspend is made, and before any report is
// generated.
#define ZX_SYSTEM_SUSPEND_OPTION_DISCARD ((uint64_t)(1u << 0))

// ZX_SYSTEM_SUSPEND_OPTION_REPORT_ONLY is an option which may be passed to
// zx_system_suspend_enter which will cause the syscall to skip any attempts to
// suspend.  Instead, it will only generate a report as it unwinds.
#define ZX_SYSTEM_SUSPEND_OPTION_REPORT_ONLY ((uint64_t)(1u << 1))

// An explicit definition of the option type.  This can be removed when the fidl
// generator start to automatically generate all of the C bindings for us by
// default.
typedef uint64_t zx_system_suspend_option_t;

typedef struct zx_wake_source_report_header {
  // The timestamp of when the wake report was generated.
  zx_instant_boot_t report_time;

  // The timestamp of when the suspend operation was committed to and started,
  // or ZX_TIME_INFINITE if no attempt to suspend was made.
  zx_instant_boot_t suspend_start_time;

  // The total number of wake sources present in the system at the time of
  // report generation.
  uint32_t total_wake_sources;

  // The total number of report entries which were waiting to be reported at the
  // start of report generation, but were skipped due to insufficient space in
  // the user's buffer.
  uint32_t unreported_wake_report_entries;
} zx_wake_source_report_header_t;

// A zx_wake_source_report_entry flag indicating that the entry's wake source
// was still in the signaled (not-yet-acknowledged) state at the time of report
// generation.
#define ZX_SYSTEM_WAKE_REPORT_ENTRY_FLAG_SIGNALED ((uint32_t)(1u << 0))

// A zx_wake_source_report_entry flag indicating that this entry has been
// reported before, but is being reported again because the wake source is still
// in the signaled state, and has not been acknowledged since being previously
// reported.
#define ZX_SYSTEM_WAKE_REPORT_ENTRY_FLAG_PREVIOUSLY_REPORTED ((uint32_t)(1u << 1))

// An explicit definition of the flag type.  This can be removed when the fidl
// generator start to automatically generate all of the C bindings for us by
// default.
typedef uint32_t zx_system_wake_report_entry_flag_t;

typedef struct zx_wake_source_report_entry {
  // The KOID uniquely identifying the kernel wake-source object being reported.
  zx_koid_t koid;

  // The name of the object to assist in debugging.  This field is for debugging
  // only, it is not durable and should not be used in production logic.
  char name[ZX_MAX_NAME_LEN];

  // The time (on the boot timeline) at which this wake source first became
  // signaled.
  zx_instant_boot_t initial_signal_time;

  // The time (on the boot timeline) at which this wake source most recently
  // became signaled.  Will be equal to the initial_signal_time if the wake
  // source has only been signaled once.
  zx_instant_boot_t last_signal_time;

  // The time (on the boot timeline) at which this wake source most recently
  // became acknowledged, if ever.  If not, this will be ZX_TIME_INFINITE.
  zx_instant_boot_t last_ack_time;

  // The total number of times that this wakes source has been signaled before
  // being reported.
  uint32_t signal_count;

  // Any valid combination of the ZX_SYSTEM_WAKE_REPORT_ENTRY_FLAG`s defined
  // above.
  zx_system_wake_report_entry_flag_t flags;
} zx_wake_source_report_entry_t;

// ====== End of wake vector reporting support //

// Options used by zx_system_barrier
#define ZX_SYSTEM_BARRIER_DATA_MEMORY ((uint32_t)0u)
#define ZX_SYSTEM_BARRIER_INSTRUCTION_STREAM ((uint32_t)1u)

#ifndef _KERNEL

#include <zircon/syscalls.h>

__BEGIN_CDECLS

// Make sure this matches <zircon/syscalls.h>.
#define _ZX_SYSCALL_DECL(name, type, attrs, nargs, arglist, prototype) \
  extern attrs type zx_##name prototype;                               \
  extern attrs type _zx_##name prototype;

#ifdef __clang__
#define _ZX_SYSCALL_ANNO(attr) __attribute__((attr))
#else
#define _ZX_SYSCALL_ANNO(attr)  // Nothing for compilers without the support.
#endif
#define _ZX_SYSCALL_EXTERN_C

#include "syscalls/gen/cdecls-next.inc"

#undef _ZX_SYSCALL_EXTERN_C
#undef _ZX_SYSCALL_ANNO
#undef _ZX_SYSCALL_DECL

__END_CDECLS

#endif  // !_KERNEL

#endif  // ZIRCON_SYSCALLS_NEXT_H_
