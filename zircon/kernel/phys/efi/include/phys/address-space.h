// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_PHYS_EFI_INCLUDE_PHYS_ADDRESS_SPACE_H_
#define ZIRCON_KERNEL_PHYS_EFI_INCLUDE_PHYS_ADDRESS_SPACE_H_

#include <stddef.h>

#include <phys/efi/page-size.h>

// A minimal EFI definition of the AddressSpace class. While the EFI
// environment is naturally virtually-addressed and no explicit mapping logic
// is needed, it serves to have the following minimal interface for use in
// general phys code.
class AddressSpace {
 public:
  static constexpr size_t kPageSize = kEfiPageSize;
};

#endif  // ZIRCON_KERNEL_PHYS_EFI_INCLUDE_PHYS_ADDRESS_SPACE_H_
