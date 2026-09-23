# Bootfs launching performance benchmark

This test is intended to measure the performance of launching many ELF
components concurrently from the bootfs. It is meant to cover the following
use cases:

- Fuchsia needs to boot fast in a resource constrained environment.
- Fuchsia needs to run many ELF test components in parallel.

The test builds a custom Zircon Boot Image (ZBI) with a bootfs that contains a
few synthesized ELF components. Each ELF component will report back when it hits
the first line of `main()`. The test will measure the latency from starting
`component_manager` to when all ELF components has reached `main()`.

