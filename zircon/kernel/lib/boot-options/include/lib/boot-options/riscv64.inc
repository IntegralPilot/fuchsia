// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// This file is #include'd multiple times with the DEFINE_OPTION macro defined.
// See kernel/lib/boot-options/README.md for details.
//
// These are the riscv64-specific boot options.

// TODO(https://fxbug.dev/42160387): Unify arch agnostic options.
DEFINE_OPTION("kernel.smp.maxcpus", uint32_t, riscv64_smp_max_cpus, {16}, R"""(
This option caps the number of CPUs to initialize.  It cannot be greater than *SMP\_MAX\_CPUS*
for a specific architecture.

Note: The default value may vary for each architecture.
)""")

DEFINE_OPTION("kernel.riscv64.enable-asid", bool, riscv64_enable_asid, {true}, R"""(
This enables use of ASIDs. True by default if the underlying hardware supports 16-bit ASIDs.
)""")

DEFINE_OPTION("kernel.riscv64.phys.mmu", bool, riscv64_phys_mmu, {true}, R"""(
This enables use of the MMU in the kernel's early boot phase.
)""")
