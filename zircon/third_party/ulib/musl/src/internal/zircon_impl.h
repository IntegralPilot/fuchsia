#pragma once

#include <zircon/process.h>
#include <zircon/types.h>

// Elide the internal versions in unit test code.
#if !defined(LIBC_NAMESPACE) || defined(LIBC_COPT_PUBLIC_PACKAGING)

#include "libc.h"

extern zx_handle_t __zircon_process_self ATTR_LIBC_VISIBILITY;
extern zx_handle_t __zircon_vmar_root_self ATTR_LIBC_VISIBILITY;
extern zx_handle_t __zircon_job_default ATTR_LIBC_VISIBILITY;
extern zx_handle_t __zircon_namespace_svc ATTR_LIBC_VISIBILITY;

#define _zx_process_self() (__zircon_process_self + 0)
#define _zx_vmar_root_self() (__zircon_vmar_root_self + 0)
#define _zx_job_default() (__zircon_job_default + 0)

#endif  // !defined(LIBC_NAMESPACE) || defined(LIBC_COPT_PUBLIC_PACKAGING)
