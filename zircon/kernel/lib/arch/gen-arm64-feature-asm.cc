// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/arm64/feature.h>

#include <hwreg/asm.h>

int main(int argc, char** argv) {
  return hwreg::AsmHeader()  //
      .Register<arch::ArmIdAa64IsaR0El1>("ID_AA64ISAR0_EL1_")
      .Register<arch::ArmIdAa64IsaR1El1>("ID_AA64ISAR1_EL1_")
      .Register<arch::ArmIdAa64IsaR2El1>("ID_AA64ISAR2_EL1_")
      .Register<arch::ArmIdAa64Pfr0El1>("ID_AA64PFR0_EL1_")
      .Register<arch::ArmIdAa64Pfr1El1>("ID_AA64PFR1_EL1_")
      .Register<arch::ArmIdAa64Mmfr0El1>("ID_AA64MMFR0_EL1_")
      .Register<arch::ArmIdAa64Mmfr1El1>("ID_AA64MMFR1_EL1_")
      .Register<arch::ArmIdAa64Mmfr2El1>("ID_AA64MMFR2_EL1_")
      .Register<arch::ArmIdAa64Mmfr3El1>("ID_AA64MMFR3_EL1_")
      .Main(argc, argv);
}
