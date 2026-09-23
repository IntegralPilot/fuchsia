// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_INCLUDE_ARCH_CRASHLOG_H_
#define ZIRCON_KERNEL_INCLUDE_ARCH_CRASHLOG_H_

#include <stdio.h>

#include <arch/crashlog_regs.h>

// Render the architecture specific details of an iframe_ in a fashion suitable
// for a crashlog into the specified FILE target.  Usually, this is just a
// register dump, but might also include other important things (stuff like the
// memory immediately surrounding the instruction which triggered the fault).
void arch_render_crashlog_registers(FILE& target, const crashlog_regs_t& iframe);

#endif  // ZIRCON_KERNEL_INCLUDE_ARCH_CRASHLOG_H_
