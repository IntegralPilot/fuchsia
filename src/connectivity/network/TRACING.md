# Tracing the netstack

If you are using Netstack2 as the system network stack (the current default for
every product except those on RISC-V), you can enable tracing for the netstack
by setting the `enable_netstack2_tracing` GN arg to `true`. If you are using
Netstack3, you don't have to do anything to enable tracing: Netstack3 has
tracing support compiled in by default.

Ensure that your `fx set` includes `--with-base //bundles/tools`, otherwise
Netstack will fail to connect to `trace_manager` at initialization time.

Make sure to rebuild and reboot your emulator or OTA/flash your device to
ensure the system picks up the changes.

## How to run a trace

You can use the `ffx trace` subcommand (see [docs][ffx-trace]) to run a trace on
a running Fuchsia system (e.g. an emulator, smart display, or NUC) that is
connected to your development host machine. For example:

```
ffx trace start --categories net,kernel:sched --duration 10
```

This will record a trace of all categories in the `kernel:sched` and `net`
categories. (`kernel:sched` includes detailed scheduler information, and `net`
includes all events emitted by the netstack.) You might want to record this
trace while doing something that exercises the netstack on the Fuchsia device;
for example, running iperf3.

## How to view the trace results

When you run `ffx trace`, it will produce a trace file that is viewable in
[Perfetto](https://ui.perfetto.dev/).

[ffx-trace]: https://fuchsia.dev/fuchsia-src/development/sdk/ffx/record-traces
