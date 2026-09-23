<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0234" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
<!-- mdformat on -->

## Summary

Here we propose updating our RISC-V ABI to be given by the RVA22{U,S}64 profiles
with added support for the [Vector extension (V)][v-ext].

## Motivation

Representing industry consensus by way of the RISC-V International foundation,
RVA22 gives the current conception of a reasonable baseline - for hardware and
software alike - when targeting application-level processors. Given the
expectation of widespread support in the emerging generation of RISC-V hardware
and the fact that Fuchsia has not formally adopted support for any hardware in
the last generation, the bar for Fuchsia's adoption of RVA22 at this time is
low.

The rationale we gave previously for the initial, provisional adoption of
RVA20{U,S}64 in [RFC-0211][rfc-0211] was that it was supported by QEMU - our
sole, official target - in our early, pre-hardware support phase for RISC-V;
consequently, we are still in that phase and QEMU now supports the additional
RVA22 extensions, so the consideration here should be similar.

While the Vector extension (V) is optional in RVA22, it is set to become
[mandatory in RVA23][rva23], so the same logic above will soon apply again.
Further, the extension is needed for parity with our supported architectures.
On x86-64, we support the SSE and AVX2 extensions; on arm64, we support the
ASIMD(/NEON) extension; by contrast, V is the corresponding RISC-V extension
and we lack support for it. SIMD/vector extensions will also be crucial to
supporting real world workloads (e.g., for graphics) at any reasonable
fidelity.

## Stakeholders

*Facilitator:* jamesr@google.com

*Reviewers:*

-   phosek@google.com *(Toolchain)*
-   travisg@google.com *(Kernel)*

## Design

One important point of design is how we approach the variable-length nature of
the vector registers when it comes to managing and exposing its state: as
spec'd, they can each range from 16 bytes to 8KiB in length each, which makes
the total size of the vector state range from 1/2 KiB bytes to 256 KiB. However,
since the foreseeable vector extension implementations are limited to 16 byte
lengths, we can make that simplifying assumption that they are exactly 16 bytes
in length for now and punt on variability.

Apart from that, there is thankfully little to design. Clang, GCC, and `rustc`
already support the ISA extensions in question, and opting-in amounts to a
simple build change. Kernel support will follow its pre-established patterns for
dealing with floating point/vector state and is not material to this proposal.

## Implementation

*   Our QEMU command line will be updated to opt into emulating the new
    instructions and features.
*   RVA22 will be set as the new ABI with a simple build change, by setting
    `fuchsia_riscv_profile = riscv_profiles.rva22`
    [here][profile-build-setting].
*   Extending the ABI again with V support will done in conjunction with adding
    kernel support for the saving of vector register state.
*   `zx_thread_read_state(..., ZX_THREAD_STATE_VECTOR_REGS,...)` for RISC-V will
    be updated to return the thirty-two 16 byte vector register contents (at
    which point debuggers may begin to add support of their own).
*   Once stable, we will then change the Clang compiler driver to use a default
    `-march` value for the `riscv64-fuchsia` target that matches the new ABI
    (propagating the change to all users of Fuchsia, and not just to those
    operating within the platform's build system).

## Performance

The bit manipulation instructions added in Zb{a,b,s} should result in fewer
instructions and accelerate a large class of basic arithmetic.

Allowing our compilers to emit vector instructions should result in a net
increase in performance (trusting compiler heuristics, which should only improve
in time).

While we are lengthening the amount of work done within each context switch with
the saving of vector state, it will amount to copying only 512 bytes, and there
are low-hanging optimizations on offer to avoid this copy when a thread does not
modify the vector state (which may be often).

## Ergonomics

This proposal avoids conditional implementation in the kernel and vDSO of cache
maintenance (courtesy of Zicbo{m,p,z}), as well as runtime selection in
userspace around hand-tuned vector routines.

## Backwards Compatibility

The proposed ABI change is simply additive (and we do not currently deal in
distributed RISC-V software), so there are no concerns on this front.

## Security considerations

N/A

## Privacy considerations

N/A

## Testing

A Zircon core test will be added to demonstrate that vector computation
contrived to execute across multiple context switches will result in the
expected value. This indirectly demonstrates that the kernel saves the relevant
vector state across context switches.

## Documentation

The current precedent (set by past RFCs that have defined or bumped
architecture-specific ABIs) is that this RFC itself serves as the documented
baseline.

## Drawbacks, alternatives, and unknowns

We are trying to strike a balance between the convenience of Fuchsia adopting a
wider variety of hardware platforms - by mandating fewer features - and the
convenience of application developers to target Fuchsia without additional
burden - by offering the same features as other operating systems. However we
do not know what the relevant hardware platforms and application ecosystems will
ultimately settle on. Until then, RVA22 (the current generation profile) with V
(a soon-to-be-mandatory extension enabling a fundamental mode of computation)
does not seem like an imprudent next baseline.

A somewhat simpler alternative would just be to mandate RVA22. However, as
mentioned above, the desire to stick to a single RISC-V standard will already
imply V soon enough with RVA23. With optional V support, we would also need to
introduce new support in the kernel and userspace for feature detection and
runtime switching, an approach that trades off wider hardware support for
runtime complexity.

## Prior art and references

Android is in the process of [(tentatively) setting][android-tentative-abi]
their ABI as RVA22 + V, as well as some Vector Crypto extensions.

[android-tentative-abi]: https://youtu.be/xLwdUn3DQp8?feature=shared&t=16m30s
[profile-build-setting]: https://cs.opensource.google/fuchsia/fuchsia/+/main:build/config/riscv64/riscv64.gni;l=153;drc=0d68b683e16176330ec4a19f326d5d8bc2ab61dd
[rfc-0211]: /docs/contribute/governance/rfcs/0211_fuchsia_on_risc-v.md
[riscv-profiles]: https://github.com/riscv/riscv-profiles/blob/fe6d34eb871c4397dc1a5245edfa1293b869f755/profiles.adoc
[rva23]: https://github.com/riscv/riscv-profiles/blob/fe6d34eb871c4397dc1a5245edfa1293b869f755/rva23-profile.adoc
[v-ext]: https://github.com/riscv/riscv-v-spec/blob/master/v-spec.adoc
