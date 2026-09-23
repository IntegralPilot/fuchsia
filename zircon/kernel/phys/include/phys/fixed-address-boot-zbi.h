// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_PHYS_INCLUDE_PHYS_FIXED_ADDRESS_BOOT_ZBI_H_
#define ZIRCON_KERNEL_PHYS_INCLUDE_PHYS_FIXED_ADDRESS_BOOT_ZBI_H_

#include <zircon/assert.h>

#include <ktl/optional.h>

#include "boot-zbi.h"

// TODO(https://fxbug.dev/408020980): Rename to <phys/fixed-address-boot-zbi.h>
//
// This BootZbi subclass provides support for booting ZBI kernels at a fixed
// load address. It requires that the provided current locations of the kernel
// and data do not overlap with their respective intended load addresses.
class FixedAddressBootZbi : public BootZbi {
 public:
  using BootZbi::Error;

  // Inits a default constructed object. Just like |BootZbi::*| but performs additional
  // initialization depending on the zbi format. (Fixed or position independent entry address).
  fit::result<Error> Init(InputZbi zbi);
  fit::result<Error> Init(InputZbi zbi, InputZbi::iterator kernel_item);

  uint64_t KernelEntryAddress() const { return kernel_entry_address_; }

  bool MustRelocateDataZbi() const {
    return kernel_load_address_ && FixedKernelOverlapsData(kernel_load_address_.value());
  }

  fit::result<Error> Load(uint32_t extra_data_capacity = 0,
                          ktl::optional<uint64_t> kernel_load_address = ktl::nullopt,
                          ktl::optional<uint64_t> data_load_address = ktl::nullopt);

  [[noreturn]] void Boot(ktl::optional<void*> argument = {});

  void Log();

 private:
  // class Trampoline; (removed)

  void set_kernel_load_address(uint64_t load_address) {
    kernel_load_address_ = load_address;
    kernel_entry_address_ = load_address + KernelHeader()->entry;
  }

  void LogFixedAddresses() const;

  // Must be called after BootZbi::Init and before Load.
  void SetKernelAddresses();

  ktl::optional<uint64_t> kernel_load_address_;
  ktl::optional<uint64_t> data_load_address_;
  uint64_t kernel_entry_address_ = 0;
};

#endif  // ZIRCON_KERNEL_PHYS_INCLUDE_PHYS_FIXED_ADDRESS_BOOT_ZBI_H_
