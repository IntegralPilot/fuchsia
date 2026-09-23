// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_PHYS_EFI_INCLUDE_PHYS_EFI_EFI_BOOT_ZBI_H_
#define ZIRCON_KERNEL_PHYS_EFI_INCLUDE_PHYS_EFI_EFI_BOOT_ZBI_H_

#include <lib/zbitl/efi.h>
#include <lib/zbitl/view.h>

#include <phys/boot-zbi.h>
#include <phys/efi/file.h>

class EfiBootZbi : public BootZbi {
 public:
  using EfiFileZbi = zbitl::View<EfiFilePtr>;

  explicit EfiBootZbi(EfiFilePtr file) : file_zbi_(std::move(file)) {}

  // This returns a fit::result<Zbi::Error, Zbi>(uint32_t) callable.
  // It performs ZBI loading using the EFI File Protocol.
  auto LoadFunction() {
    return [this](uint32_t extra_capacity) { return Load(extra_capacity); };
  }

  // This returns a fit::result<Zbi::Error>(Zbi) callable.
  // It installs the data ZBI and does the final logging before Boot().
  auto LastChanceFunction() {
    return [this](const Zbi& zbi) { return LastChance(zbi); };
  }

  // This returns a [[noreturn]] void() callable to perform the actual boot.
  // It does nothing else.
  auto BootFunction() {
    return [this]() { Boot(); };
  }

 private:
  fit::result<Zbi::Error, Zbi> Load(uint32_t extra_data_capacity);

  fit::result<Zbi::Error> LastChance(const Zbi& zbi);

  EfiFileZbi file_zbi_;
};

#endif  // ZIRCON_KERNEL_PHYS_EFI_INCLUDE_PHYS_EFI_EFI_BOOT_ZBI_H_
