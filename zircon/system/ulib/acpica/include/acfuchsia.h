// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ACFUCHSIA_H_
#define ACFUCHSIA_H_

#include <stdbool.h>
#include <threads.h>
#include <zircon/assert.h>
#include <zircon/syscalls.h>

/*
 * Settings described in section 7 of
 * https://acpica.org/sites/acpica/files/acpica-reference_17.pdf
 */

#if __x86_64__
#define ACPI_MACHINE_WIDTH 64
#elif __aarch64__
#define ACPI_MACHINE_WIDTH 64
#elif __riscv && __riscv_xlen == 64
#define ACPI_MACHINE_WIDTH 64
#else
#error Unexpected architecture
#endif

extern zx_paddr_t acpi_rsdp;
extern zx_handle_t mmio_resource_handle;
extern zx_handle_t ioport_resource_handle;
extern zx_handle_t irq_resource_handle;
extern zx_handle_t power_resource_handle;

#ifndef ACPI_OFFSET
#define ACPI_OFFSET(d, f) offsetof(d, f)
#endif

// Make this a no-op.  The only codepath we use it for is ACPI poweroff, in
// which case we don't care about the cache state.
#define ACPI_FLUSH_CPU_CACHE()

// Use the standard library headers
#define ACPI_USE_STANDARD_HEADERS
#define ACPI_USE_SYSTEM_CLIBRARY

// Use the builtin cache implementation
#define ACPI_USE_LOCAL_CACHE

#define ACPI_MUTEX_TYPE ACPI_OSL_MUTEX

typedef struct AcpiSemaphore acpi_semaphore_t;
typedef struct sync_mutex sync_mutex_t;

// Specify the types Fuchsia uses for various common objects
#define ACPI_CPU_FLAGS int
#define ACPI_SPINLOCK sync_mutex_t *
#define ACPI_MUTEX sync_mutex_t *
#define ACPI_SEMAPHORE acpi_semaphore_t *

__BEGIN_CDECLS
bool _acpica_acquire_global_lock(void *FacsPtr);
bool _acpica_release_global_lock(void *FacsPtr);

void acpica_enable_noncontested_mode(void);
void acpica_disable_noncontested_mode(void);
__END_CDECLS

#define ACPI_ACQUIRE_GLOBAL_LOCK(FacsPtr, Acq) Acq = _acpica_acquire_global_lock(FacsPtr)
#define ACPI_RELEASE_GLOBAL_LOCK(FacsPtr, Pnd) Pnd = _acpica_release_global_lock(FacsPtr)

#endif  // ACFUCHSIA_H_
