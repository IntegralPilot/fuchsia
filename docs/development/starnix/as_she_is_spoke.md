# As She Is Spoke

Starnix implements the Linux UAPI with semantics that match the Linux kernel
implementation as closely as possible. The Linux UAPI has a long and storied
history, filled with many great ideas alongside some less optimal ones. To
successfully run unmodified Linux binaries, Starnix maintains an unopinionated
stance. It implements the Linux UAPI exactly as it functions in practice, rather
than attempting to enforce an idealized design. Starnix implements the Linux
UAPI [*as She is spoke*][as-she-is-spoke]{: .external}.

This approach simplifies debugging. When a Linux program running under Starnix
behaves incorrectly, the cause falls into one of two categories:

*   **The program relies on behavior not supported by Linux**: If the program
fails identically on the Linux kernel, the issue lies within the program, not
Starnix.

*   **Starnix implements the UAPI inaccurately**: If the program works on the
Linux kernel but fails on Starnix, a Starnix will be able to run the program
correctly if Starnix is changed to implement the UAPI more accurately.

## Risks of novel semantics

Exposing features or behaviors to Linux userspace that do not exist in the Linux
kernel violates the principle of exact compatibility. Introducing
Starnix-specific devices, file systems, syscalls, or pseudo-files creates a risk
of divergence.

This divergence creates a compatibility conflict: one application may rely on
standard Linux semantics, while another inadvertently relies on Starnix-specific
behavior. In this scenario, aligning Starnix with the Linux kernel to fix the
first application could break the second.

Exposing implementation details, such as how Starnix uses underlying Fuchsia
primitives to implement the Linux UAPI, is risky because these details often
must change to improve semantic accuracy. For example, while a one-to-one
correspondence might currently exist between a Linux and Fuchsia concept, future
developments might require a more flexible implementation to fully match the
nuances of the Linux concept.

## Exceptions and Caveats

There are limits to this general approach. Starnix adheres to the Linux UAPI
where possible, but functional necessities and security requirements dictate
some deviations.

*   **Fuchsia integration:** Starnix exposes `remotefs`, a Fuchsia-specific file
system, to Linux userspace. Because `fxfs` must be exposed to the system,
Starnix exposes it through `remotefs` rather than pretending that `fxfs` is
`ext4` or another specific Linux file system; masquerading as another real file
system type would cause significant implementation difficulties. Starnix also
exposes some Fuchsia-specific devices, such as the `magma` device, which is
necessary for graphics performance.

*   **Security exclusions:** Starnix refrains from implementing extremely
dangerous aspects of the Linux UAPI. For example, `/dev/mem`, which contains the
physical memory of the device, is not implemented because it presents a major
security hazard.

*   **Feature evolution:** In some cases, features were initially omitted but
later added. For example, Starnix initially refrained from implementing `suid`,
but the feature was eventually added to support important Linux programs that
required the standard Linux kernel semantics.

[as-she-is-spoke]: https://en.wikipedia.org/wiki/English_as_She_Is_Spoke
