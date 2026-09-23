// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/riscv64/system.h>

#include <hwreg/asm.h>

int main(int argc, char** argv) {
  return hwreg::AsmHeader()  //
      .Register<arch::RiscvSstatus>("SSTATUS_")
      .Register<arch::RiscvStvec>("STVEC_")
      .Register<arch::RiscvScause>("SCAUSE_")
      .Main(argc, argv);
}
