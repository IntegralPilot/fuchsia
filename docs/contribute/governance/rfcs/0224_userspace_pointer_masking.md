<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0224" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

## Summary

This document proposes changes needed to support the RISC-V J-extension pointer
masking feature in Fuchsia userspace.

## Motivation

The [RISC-V J-extension][jext] aims to make RISC-V an attractive target for
languages that are traditionally interpreted or JIT compiled, or which require
large runtime libraries or language-level virtual machines. Examples include
(but are not limited to) C#, Go, Haskell, Java, JavaScript, OCaml, PHP, Python,
R, Ruby, Scala, Smalltalk or WebAssembly. One notable feature in J-extension is
[pointer masking (PM)][pm]. This is a hardware feature that, when enabled,
allows the MMU to ignore the top N bits of the effective address on memory
accesses. This is very similar to the [Top-Byte-Ignore (TBI)][tbi] feature on
ARMv8.0 CPUs. One of the immediate uses of PM is enabling [Hardware-assisted
AddressSanitizer (HWASan)][hwasan] in userspace, where tags are stored in the
top byte for memory tracking.

## Terminology

Much of the terminology used in this document can be extended from [RFC-0143:
Userspace Top-Byte-Ignore][rfc143].

__Address__ - An address is a 64-bit integer that represents a location within
the bounds of a user address space. An address is never tagged.

__Pointer__ - A location of dereferenceable memory which may or may not have a
tag. This is equivalent to the effective address as defined in the RISC-V Base
ISA.

__Tag__ - The upper bits of a pointer, generally used for metadata. RISC-V
pointer masking supports different tag sizes.

__Zjpm__ - This is the formal identifier of the pointer masking feature in
J-extension.

## Design

### Conforming to the Tagged Pointer ABI

Zjpm enables tagged userspace pointers. Handling of these pointers by the
kernel are subject to the same rules dictated in [RFC-0143][rfc143]. That is:

1. The kernel will ignore tags on user pointers received from syscalls.

2. It is an error to pass a tagged pointer on syscalls that accept addresses.

3. When the kernel accepts a tagged pointer, whether through syscall or fault,
   it will try to preserve the tag to the degree that user code may later
   observe it.

4. The kernel itself will never generate tagged pointers.

5. When comparing userspace pointers, the kernel will ignore any tags that may
   be present.

Additionally, Zjpm will be controlled by a kernel boot-option. Similar to ARM
TBI, Zjpm will be on for all userspace processes when enabled.

RISC-V provides no semantic meaning to values written to a debug register, so
debuggers can freely write a tagged value to debug registers. For features such
as watchpoints, pointer comparison is controlled via the `match` field of the
`mcontext6` register, which can be controlled to either match an exact tagged
value or ignore the tag just like with pointer masking.

### Set the tag to 8 bits (for Sv39 and Sv48)

The most immediate use case of Zjpm is enabling memory error detection tools
such as HWASan on RISC-V which is dependent on storing metadata into the top
bits of a pointer. ARM TBI only supports a tag size of 8 bits. Zjpm is much
more flexible though, allowing a variable number of top bits to be ignored of
memory accesses. This number of bits can be controlled in different modes via a
CSR register.

The simplest approach is to just conform to what we have now. ARM TBI and
HWASan already use an 8-bit tag but there really isn't any immediate or
foreseeable demand for a tag larger (or smaller) than 8 bits.

A tag size of 8 bits will only be supported for Sv39 and Sv48. This is because
Sv57 only supports masking the top 7 bits. The tag size will need to be
revisited if Sv57 is supported in the future.

## Implementation

Much of the implementation will be very similar to the implementation for ARM
TBI. Zjpm can be enabled in U-mode by setting the `uenable` bit in the `upm`
CSR. The tag size can be set via the `ubits` bitfield in `upm`.

The existing syscall infrastructure should already be set up for accepting
tagged userspace pointers with a fixed tag size.

The `ZX_FEATURE_KIND_ADDRESS_TAGGING` [feature][features] will have an extra
flag (something like `ZX_RISCV64_FEATURE_ADDRESS_TAGGING_ZJPM_8BIT`) to
indicate Zjpm is enabled and the tag size.
`ZX_RISCV64_FEATURE_ADDRESS_TAGGING_ZJPM_8BIT` indicates that the top 8 bits
are definitely masked, but in the future we may want to add other flags that
mean more of the upper bits get masked. For example, we could add something
like `ZX_RISCV64_FEATURE_ADDRESS_TAGGING_ZJPM_16BIT` to indicate the top 16
bits are masked, but `ZX_FEATURE_KIND_ADDRESS_TAGGING` would also set the 8-bit
equivalent flag to ensure compatible code checking the 8-bit flag will work for
future systems.

Zjpm also depends on the Zicsr extension being enabled, which provides
instructions for modifying CSR registers.

## Performance

Performance impact should be negligible and existing microbenchmarks will be
used to verify.

## Testing

The same suite of tests used for testing ARM TBI should also be applied to
Zjpm. These tests should be largely agnostic of which address tagging mode is
enabled.

## Drawbacks, alternatives, and unknowns

Zjpm provides much more flexibility than ARM TBI, so we could support more
masking options.

At the moment, there doesn't seem to be any real hardware that supports the
latest version of Zjpm, so it's underspecified how the kernel will be able to
discover what support is available in the hardware. Until we know what real
constraints there will be for hardware, the conservative single-feature
always-on mode proposed here will be what's best supported if anything is.
[QEMU][qemu] should support pointer masking and can be enabled with the
"x-j" cpu property.

### Set the tag to some other static value always

The tagging ABI allows room for supporting different tag sizes (that is, we
aren't restricted to 8 bits). In practice, we don't really use much of the
top bits of a virtual address. On x86, we effectively only use the bottom 48
bits, although this is just an assumption made for what we target now and
subject to change in the future. Currently, the bit field that indicates the
tag size is 5 bits long, meaning only up to the top 31 bits of a pointer can be
ignored. The remainder of the bits above this field are WPRI, so this field
could be expanded for larger values in the future.

For tools like HWASan, the memory tagging algorithm isn't necessarily
dependent on the tag size being 8 bits. Increasing the tag size to something
like 16 bits can significantly reduce the chances of a false positive in tag
comparisons, although that would mean needing to store a larger tag into shadow
memory which isn't very desirable. The current false positive probability with
8 bits is also very small already.

### Expose the tag as a modifiable value to userspace

This option implies a method of exposing the pointer masking feature to users.
That is, users can (1) enable/disable pointer masking at runtime and (2) users
can change the tag size. This might not be as desirable since there isn't an
immediate need to do this and it would require adding more syscalls for toggling
these values. The tagging ABI provides room though for supporting this in the
future.

### Pointer Masking on Instruction Fetching

One powerful feature of Zjpm is enabling PM on instruction fetches, including
those resulting from monotonic PC increases due to straight line execution,
control transfers (e.g., branches and direct/indirect jumps and
`uret`/`sret`/`mret`). This proposal only outlines pointer masking rules on
data pointers, but leaves room for exploring this option in the future.

### Interaction with other desirable hardware features

Zjpm only introduces pointer masking functionality. Other useful features like
tag checking or sandbox enforcement may be implemented in either software or
future hardware extensions that require Zjpm. An example analogous feature is
ARM MTE which depends on TBI.

### Changes to the draft

Zjpm is still currently a draft proposal, but there's desire to see it ratified
for RVA23 so HWASan can formally support it. This document will be updated to
accommodate any major changes in the spec.

[jext]: https://github.com/riscv/riscv-j-extension
[pm]: https://github.com/riscv/riscv-j-extension/blob/master/zjpm-spec.pdf
[tbi]: https://developer.arm.com/documentation/den0024/a/ch12s05s01
[hwasan]: https://clang.llvm.org/docs/HardwareAssistedAddressSanitizerDesign.html
[rfc143]: /docs/contribute/governance/rfcs/0143_userspace_top_byte_ignore.md
[features]: /reference/syscalls/system_get_features.md
[qemu]: https://lists.gnu.org/archive/html/qemu-riscv/2021-10/msg00682.html
