// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_ACPI_LITE_INCLUDE_LIB_ACPI_LITE_ZIRCON_H_
#define ZIRCON_KERNEL_LIB_ACPI_LITE_INCLUDE_LIB_ACPI_LITE_ZIRCON_H_

#include <lib/acpi_lite.h>
#include <lib/zx/result.h>
#include <zircon/types.h>

#include <vm/vm_address_region.h>

namespace acpi_lite {

// Convert physical addresses to virtual addresses by creating new mappings as required.
class ZirconPhysmemReader final : public PhysMemReader {
 public:
  constexpr ZirconPhysmemReader() = default;

  zx::result<const void*> PhysToPtr(uintptr_t phys, size_t length) override;

 private:
  struct Mapping : public fbl::SinglyLinkedListable<ktl::unique_ptr<Mapping>> {
    fbl::RefPtr<VmMapping> mapping;
  };

  DECLARE_MUTEX(ZirconPhysmemReader) lock_;
  fbl::SinglyLinkedList<ktl::unique_ptr<Mapping>> mappings_ TA_GUARDED(lock_);
};

// Create a new AcpiParser, starting at the given Root System Description Pointer (RSDP),
// and using Zircon's |paddr_to_physmap| implementation to convert physical addresses
// to virtual addresses.
zx::result<AcpiParser> AcpiParserInit(zx_paddr_t rsdp_pa);

}  // namespace acpi_lite

#endif  // ZIRCON_KERNEL_LIB_ACPI_LITE_INCLUDE_LIB_ACPI_LITE_ZIRCON_H_
