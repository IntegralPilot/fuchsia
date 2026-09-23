// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <dev/init.h>
#include <phys/arch/arch-handoff.h>

void PlatformDriverHandoffEarly(const ArchPhysHandoff& arch_handoff) {}
void PlatformDriverHandoffPostVm(const ArchPhysHandoff& arch_handoff) {}
void PlatformDriverHandoffLate(const ArchPhysHandoff& arch_handoff) {}
