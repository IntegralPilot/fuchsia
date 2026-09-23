// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/asm.h>
#include <phys/zbi.h>

// The linker script (phys.ld) places this section first in the image.  It
// provides the ZBI headers for the kernel so that the whole load image is
// a standalone ZBI container of one (ZBI_TYPE_KERNEL_*) item.
.section .boot.header, "a", %progbits

.zbi.kernel.image.object kZbiContainerHeader, end=PHYS_LOAD_END

// The zbi-header.ld linker script fragment synthesizes the zbi_kernel_t
// contents and places them directly after .boot.header so it appears here.
// This cannot be done in assembly without generating relocations that would
// make the linker try to generate dynamic fixups under -pie.
.label kZbiKernelHeader, type=object
.size kZbiKernelHeader, 16
