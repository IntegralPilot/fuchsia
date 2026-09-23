# Install Fuchsia on a device

The Fuchsia platform can be installed on the following hardware devices:

- [Intel NUC (Next Unit of Computing) devices][install-fuchsia-on-nuc]
- [Khadas VIM3 board][install-fuchsia-on-vim3]

## Architecture support

Fuchsia supports two ISAs (Instruction Set Architectures):

* `arm64` - Fuchsia supports `arm64` (also called AArch64) with no restrictions on
  supported microarchitectures.

* `x86-64` - Fuchsia supports `x86-64` (also called IA32e or AMD64), but with some
  restrictions on supported microarchitectures.

## CPU support

Fuchsia's support for CPUs:

* Intel - For Intel CPUs, only Broadwell and later are actively supported and will
  have new features added for them.  Additionally, we will accept patches to keep
  Nehalem and later booting.

* AMD - AMD CPUs are **not** actively supported (in particular, we have no active testing
  on them), but we will accept patches to ensure correct booting on them.

## Table of contents

- [Install Fuchsia on a NUC][install-fuchsia-on-nuc]
- [Install Fuchsia on a Khadas VIM3 board][install-fuchsia-on-vim3]

<!-- Reference links -->

[install-fuchsia-on-nuc]: /docs/development/hardware/intel_nuc.md
[install-fuchsia-on-vim3]: /docs/development/hardware/khadas-vim3.md
