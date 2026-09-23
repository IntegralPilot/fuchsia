## Mapped clock

This is a rust library implementing a safe API for a clock backed by memory
mapped into this process' virtual address space.  See `MappedClock` for
details.

To create one, you will need a `zx::Clock`, a `zx::Vmar` and a call to
`MappedClock::try_new`.

A memory mapped clock can be read more efficiently than a regular kernel clock
object in contexts where calling into the kernel is undesirable. At the same
time, updates to the memory mapped clock can be observed consistently with any
other observers of the same underlying clock, a property guaranteed by Zircon.

As a tradeoff, a memory mapped clock may offer a restricted set of methods,
and has more complex construction and lifecycle as compared to [zx::Clock].

## Benchmarks

When making changes, ensure that you verify the performance with the benchmarks.

See [benchmarks README.md][br] for details.

[br]: benchmarks/README.md

