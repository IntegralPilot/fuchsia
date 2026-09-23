// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-options/boot-options.h>
#include <lib/uart/all.h>
#include <lib/zbitl/view.h>

#include <ktl/utility.h>
#include <phys/boot-options.h>
#include <phys/early-boot.h>
#include <phys/main.h>
#include <phys/stdio.h>
#include <phys/uart-console.h>

#include <ktl/enforce.h>

void PhysMain(void* zbi_ptr, arch::EarlyTicks ticks) {
  // Apply any relocations required to ourself.
  ApplyRelocations();

  // Perform any architecture-specific set-up dependent on machine state.
  ArchSetUpCpu();

  // Initially set up stdout to write to nowhere.
  InitStdout();

  static BootOptions boot_options;
  InstallBootOptions(&boot_options);

  // Move default uart into the boot options, so its properly overriden (if any) by the different
  // boot options below.
  //
  // TODO(https://fxbug.dev/42084617): To be removed when driver state handed over switches do
  // `uart::all::Config` as we work to remove `uart()` accessor.
  boot_options.serial = uart::all::GetConfig(ktl::move(GetUartDriver()).TakeUart());

  // Construct a view into our ZBI suitable for early data access while the
  // data cache is possibly off and dirty.
  EarlyBootZbiBytes early_zbi_bytes{zbi_ptr};
  EarlyBootZbi early_zbi{&early_zbi_bytes};

  // Obtain proper UART configuration from ZBI, both cmdline items and uart driver items.
  SetBootOptions(boot_options, early_zbi);

  // Configure the selected UART.
  //
  // Note we don't do this after parsing ZBI items and before parsing command
  // line options, because if kernel.serial overrode what the ZBI items said,
  // we shouldn't be sending output to the wrong UART in between.
  SetUartConsole(boot_options.serial);

  // Perform any architecture-specific set-up dependent on ZBI state.
  ArchSetUpZbi(early_zbi);

  // Call the real entry point now that it can use printf!  It does not return.
  ZbiMain(zbi_ptr, early_zbi, ticks);
}
