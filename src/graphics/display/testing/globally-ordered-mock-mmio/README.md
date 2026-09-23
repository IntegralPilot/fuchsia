# MMIO mock for Rust drivers

This library is a globally-ordered mock (test double) for memory-mapped I/O
(MMIO) driver testing in Rust.

## Usage

[The crate documentation](src/lib.rs) has a full API usage guide and examples.

## Design trade-offs

* **Thread safety**: Accesses from all threads are checked against a single
  global expectation list, in the order in which they reach the mock. The mock
  does not order accesses issued by different threads; tests exercising
  concurrent drivers must establish their own ordering. The mock's internal
  lock may also mask thread synchronization issues in the code under test.
* **Optimized for reading**: Test expectations resemble an annotated
  bus-analyzer trace.
* **High information density**: Dense, line-by-line expectations with minimal
  visual punctuation noise.
* **Typed register integration**: Expectation variants that extract register
  offsets and operand widths from `mmio::register!` traits (`Register`,
  `ReadableRegister`, `WritableRegister`, `IndexedRegister`).
* **Polling support**: First-class support for deterministic sequence polling as
  well as indefinite timeout retry loops without secondary fakes.
* **Actionable messages on failure**: Each expectation's source location is
  tracked so it can be shown on failures.

## Implementation status

The library's public API was carefully reviewed and is intended to be stable.

The high-level implementation design (module and class breakdown) was reviewed.
Human developers should be able to understand and evolve the implementation.

The function-level implementation and the automated tests were reviewed. The
failure message formatting in `src/formatting.rs` is the most likely part to be
revised as we get experience with the library.
