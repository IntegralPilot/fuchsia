A cli tool to inspect .fxt files

Usage:


# Locating invalid records
```
# e.g. to locate invalid records in a trace
fx fxtdump dump trace.fxt --strict
...
...
0x0d9ffb40: Event(EventRecord { provider: Some(Provider { id: 61, name: "starnix_kernel.cm" }), timestamp: 935523250156, process: ProcessKoid(46458), thread: ThreadKoid(46460), category: "kmem_stats_a", name: "starnix:pager", args: [Arg { name: "name", value: String("write") }], payload: DurationEnd })
ERROR couldn't parse string as utf-8
```
Will get you the offset into the trace directly before where which we encountered a corrupted
record.

# Inspecting Trace Size
```
$ fx fxtdump category_stats trace.fxt
+-------------------+------------+---------+
| Category          | Size       | Count   |
+-------------------+------------+---------+
| kernel:vm         | 127451 KiB | 1812649 |
+-------------------+------------+---------+
| kernel_sched      | 49889 KiB  | 1069634 |
+-------------------+------------+---------+
| kernel:contention | 33571 KiB  | 715898  |
+-------------------+------------+---------+
| kernel:power      | 28322 KiB  | 517899  |
+-------------------+------------+---------+
| gfx               | 7009 KiB   | 241187  |
+-------------------+------------+---------+
| magma             | 2089 KiB   | 74046   |
+-------------------+------------+---------+
| input             | 948 KiB    | 38862   |
+-------------------+------------+---------+
| kernel:meta       | 188 KiB    | 3441    |
+-------------------+------------+---------+
| memory:kernel     | 49 KiB     | 307     |
+-------------------+------------+---------+
| system_metrics    | 14 KiB     | 420     |
+-------------------+------------+---------+
```
