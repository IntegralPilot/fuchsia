// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/x86/system.h>

#include <phys/address-space.h>

#include <ktl/enforce.h>

void AddressSpace::ArchInstall() const {
  auto cr4 = arch::X86Cr4::Read();

  // Disable global pages before installing the new root page table to ensure
  // that any prior global TLB entries are flushed.
  cr4.set_pge(0).Write();

  // Set the new page table root. This will flush the TLB.
  arch::X86Cr3::Write(root_paddr());

  // Now that we have installed the new root, re-enable global pages. Doing
  // this before the installation could result in more stale TLB entries.
  cr4.set_pge(1).Write();
}
