// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <stddef.h>

#include <hwreg/asm.h>
#include <phys/arch/arch-phys-info.h>

int main(int argc, char** argv) {
  return hwreg::AsmHeader()  //
      .Macro("ARCH_PHYS_INFO_BOOT_HART_ID", offsetof(ArchPhysInfo, boot_hart_id))
      .Main(argc, argv);
}
