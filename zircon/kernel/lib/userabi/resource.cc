// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/userabi/userboot.h>
#include <zircon/assert.h>

#include <ktl/utility.h>
#include <object/handle.h>
#include <object/resource_dispatcher.h>

#include <ktl/enforce.h>

HandleOwner get_resource_handle(zx_rsrc_kind_t kind) {
  char name[ZX_MAX_NAME_LEN];
  switch (kind) {
    case ZX_RSRC_KIND_MMIO:
      strlcpy(name, "mmio", ZX_MAX_NAME_LEN);
      break;
    case ZX_RSRC_KIND_IRQ:
      strlcpy(name, "irq", ZX_MAX_NAME_LEN);
      break;
#if defined(__x86_64__)
    case ZX_RSRC_KIND_IOPORT:
      strlcpy(name, "io_port", ZX_MAX_NAME_LEN);
      break;
#elif defined(__aarch64__)
    case ZX_RSRC_KIND_SMC:
      strlcpy(name, "smc", ZX_MAX_NAME_LEN);
      break;
#endif
    case ZX_RSRC_KIND_SYSTEM:
      strlcpy(name, "system", ZX_MAX_NAME_LEN);
      break;
    default:
      ZX_PANIC("userboot doesn't get zx_rsrc_kind_t %" PRIu32, kind);
      break;
  }
  zx_rights_t rights;
  KernelHandle<ResourceDispatcher> rsrc;
  zx_status_t result = ResourceDispatcher::CreateRangedRoot(&rsrc, &rights, kind, name);
  ZX_ASSERT(result == ZX_OK);
  return Handle::Make(ktl::move(rsrc), rights);
}
