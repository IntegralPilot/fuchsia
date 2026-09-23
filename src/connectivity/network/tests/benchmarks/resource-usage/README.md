The test in this directory measures the resource usage (memory consumption and
handle counts) of a hermetic netstack process while exercising it with a set of
workloads. Metrics are emitted for baseline usage on netstack startup, initial
usage after one run of a given workload, peak usage while running a workload
repeatedly, and final increase in usage from the baseline after a workload is
complete.

Each workload is meant to exercise a particular subsystem. The current workloads
are the following:
 - TCP sockets
 - UDP sockets
 - Interfaces and neighbor traffic
