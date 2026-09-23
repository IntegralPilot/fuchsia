# Zircon heap profiler

To understand how the kernel is using memory, developers can generate heap
snapshots that reveal memory allocations and their corresponding stack traces.

The Zircon Heap Profiler tracks statistics such as the number of live
allocations in memory and the total number of allocations since system start.

The data is held in a VMO shared with userspace via
`/boot/kernel/i/memory-profile/d/heap.bin`

More detail on how to use it at
src/performance/memory/heapdump/kernel-collector/README.md
