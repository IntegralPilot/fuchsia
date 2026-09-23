# Netstack benchmarks

Netstack benchmarks are a set of test components with the following objectives:

* Define and generate various netstack related benchmarking data as part of CI
  and publish them to Chromeperf benchmarking dashboards.
* Enable regression monitoring in Chromeperf to catch any regressions in CI and
  raise monorail bugs.

Our benchmarks fill a similar role to automated testing, but specifically for
performance: they give us the confidence to change and improve the network stack
while being aware of the performance impact of those changes.

Most netstack benchmarks can be run either as regular hermetic tests, or as
end-to-end performance tests using Fuchsia's perftest infrastructure. The former
is recommended for local development and iteration when possible because it is
simpler: it is compatible with any product configuration and it does not require
adding any additional dependencies to your package set.

To run the benchmarks as end-to-end tests, pass the `--e2e` flag to `fx test`
and specify one of the tests defined in //src/tests/end_to_end/perf/test. For
example:

```
fx test --e2e netstack_benchmarks_test
```

Make sure that your package universe contains the
`//src/tests/end_to_end/perf:test` group so you have the required dependencies
available. Also note that if you are running `netstack_iperf_test` or
`netstack_benchmarks_test`, you will need to run on the `terminal` product so
that SL4F is available. For example, your `fx set` line might look like the
following:

```
fx set terminal.x64 --with //src/tests/end_to_end/perf:test
```

## Microbenchmarks

### Loopback socket benchmarks {#loopback-socket-benchmarks}

These focus on measuring the duration of specific socket related system calls
from the benchmarking binary for TCP, UDP, and ICMP sockets over loopback.

#### Fake netstack

The loopback socket benchmarks also run against the "fake netstack",
a stubbed-out netstack the implements the minimum amount of the
fuchsia.posix.socket API possible, to attempt to measure the overhead of
the API structure itself.

### Tun socket benchmarks

These benchmarks measure the same operations as the
[loopback socket benchmarks](#loopback-socket-benchmarks), except there are two
netstacks involved connected via a network-tun [device-pair]. This means that
the latency numbers also include time spent in the netstack's device layer,
which is absent from the loopback benchmarks.

### UDP serde benchmarks

These benchmarks measure the time to serialize and deserialize the metadata that
is sent with each packet in Fast UDP.

### Netdevice benchmarks

These benchmarks measure the round trip latency for packet buffer transmission
between a client and a device, using an in-process fake network device.

### Netstack3 benchmarks

Netstack3 defines microbenchmarks internally that measure the following:
 * The time required to forward a batch of IP packets
 * The time required to take tokens out of a token bucket

## Macrobenchmarks

### iPerf benchmarks

These benchmarks use iperf3 to measure throughput, CPU usage, jitter, and packet
loss for TCP and UDP traffic. They run over both loopback (fully on-device) and
over Ethernet, with the server running on Fuchsia and the client on the host.
They exercise both single-flow and multi-flow scenarios.

### Resource usage benchmarks

These benchmarks measure the resource usage (memory consumption and handle
counts) of a hermetic netstack process while exercising it with a set of
workloads. Metrics are emitted for baseline usage on netstack startup, initial
usage after one run of a given workload, peak usage while running a workload
repeatedly, and final increase in usage from the baseline after a workload is
complete.

[device-pair]: /sdk/fidl/fuchsia.net.tun/tun.fidl
