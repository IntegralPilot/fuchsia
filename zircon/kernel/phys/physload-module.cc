// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-options/boot-options.h>

#include <ktl/utility.h>
#include <phys/address-space.h>
#include <phys/allocation.h>
#include <phys/boot-options.h>
#include <phys/elf-image.h>
#include <phys/main.h>
#include <phys/stdio.h>
#include <phys/symbolize.h>

#include "log.h"
#include "physload.h"

#include <ktl/enforce.h>

ArchPhysInfo* gArchPhysInfo;
AddressSpace* gAddressSpace;  // address-space.cc is not linked in.

// This transfers all the global state, so all of stdout, gLog, gSymbolize,
// BootOptions::Get(), and Allocation::GetPool(), are available just like in
// the main program that set them all up.  Then it calls PhysLoadModuleMain,
// which has the actual program logic of the particular module.
[[noreturn]] void PhysLoadHandoff(ElfImage& self, Log* log, ArchPhysInfo* arch_phys,
                                  UartDriver& uart, MainSymbolize* symbolize,
                                  const BootOptions* boot_options, memalloc::Pool& allocation_pool,
                                  AddressSpace* aspace, PhysBootTimes boot_times,
                                  KernelStorage kernel_storage) {
  // Install the log/stdout first thing.  Note this doesn't use log.SetStdout()
  // because we don't want to stash this module's (uninitialized) stdout to be
  // restored, but keep the one already stashed by physload.
  gLog = log;
  gStdout = FILE{log};

  // Install the other global state handed off from physload.
  gArchPhysInfo = arch_phys;
  gSymbolize = symbolize;
  InstallBootOptions(boot_options);
  Allocation::InitWithPool(allocation_pool);
  gAddressSpace = aspace;

  ArchOnPhysLoadHandoff();
  gSymbolize->OnHandoff(self);

  PhysLoadModuleMain(uart, boot_times, ktl::move(kernel_storage));
}
