// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/arm64/system.h>

#include "physload.h"

void ArchPhysloadBeforeInitMemory() {
  // Ensure we drop to EL1 first so that we set up our address space there so
  // we can hand that off to the kernel proper without reconstruction.
  arch::ArmDropToEl1WithoutEl2Monitor();
}
