# IPC Comparison Benchmarks

This directory contains benchmarks for comparing the performance
of different inter-process communication (IPC) methods in Fuchsia.

## Building

To add this component to your build, append
`--with-test //src/tests/benchmarks/ipc/comparison:benchmarks`
to the `fx set` invocation.

## Running

`fx test --e2e fuchsia-pkg://fuchsia.com/ipc_comparison_bench#meta/ipc_comparison_bench.cm -o`

## Benchmarks

This program benchmarks sending a large number of sized objects
over various IPC methods.

Currently the benchmark compares different methods for sending
90,000 messages that are each 2,000 bytes from a sender to a receiver.
The benchmark is completed when the receiver acknowledges receipt
of the final message.

The following methods are compared:

### Batching (`Batch/recv`)

Batch as many objects as possible in a single FIDL vector in a
single call. 30 messages are batched in each FIDL call. The method
is two-way, and the next batch is only queued when the previous
batch is acknowledged.

### Streaming (`Stream/recv`)

Format length-prefixed messages in a Zircon streaming socket. Each
message is prefixed by its length as a 32-bit unsigned integer in
little-endian order.  Sockets implement flow control, and the writer
waits until a WRITABLE signal before proceeding when a limit is
reached.

### Batched Streaming (`BatchStream/recv`)

Format length-prefixed messages in a Zircon streaming socket, but
write multiple messages in a single buffer and send them all with
a single socket send syscall.  We use the same batch size as the
`Batch/recv` benchmark for a direct comparison of socket vs. FIDL
overhead.

### Sequential (`Sequential/recv`)

Continually write objects individually through a single method call.
This is simulating the situation where we implement better flow
control for channels such that they do not terminate the calling
program when the channel is full on write.

We allow at most 128 outstanding writes in the channel without a
read.

Flow control is simulated using a shared semaphore and signalling
to determine when it is safe to write.

### VMO Batching (`VMO/recv`)

Batch as many objects as possible in batches of VMOs.

We support sending 30 VMOs in a single FIDL call, each containing
64 messages.
