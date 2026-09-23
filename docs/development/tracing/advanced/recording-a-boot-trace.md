# Recording a trace at boot time

The `ffx trace` tool can be used to configure the trace manager to start
recording a performance trace when the system is booting. This allows for capturing
performance events before the complete system is running.

## Configuring the boot time trace

The configuration of the boot time trace is done identically to any other trace
captured by `ffx trace`. The only difference is when running `ffx trace start`
add the `--on-boot` mode switch to indicate that the trace should started when
booting.

Note: It is important to use streaming or circular mode so the trace data will
be buffered to disk instead of being dropped.

Example:

```posix-terminal
ffx trace start --on-boot --buffer-size 32 --duration 120 --buffering-mode streaming
```

This will configure a trace to begin when the trace manager is initialized when
booting. It will use a buffer size of 32MB which provides additional space over
the 4MB default to capture events. The trace will stop automatically after 120
seconds.


## Checking the status of a boot trace session

The status of a trace session started when the device is booting can be checked
by using the command `ffx trace status`.

## Downloading the trace data

When the trace is completed and the system is running, the trace is downloaded using
`ffx trace stop`.

To automatically upload the trace data to Google Cloud Storage (GCS) and generate a
direct link to the Perfetto Trace Viewer, pass the `--upload` flag:

```posix-terminal
ffx trace stop --upload
```


## Recording Zircon boot trace events

The Zircon kernel's internal tracing system can be active on boot. This means performance
events created by the kernel as the system is starting can be recorded and captured
as part of a performance trace.

Enabling the Zircon boot trace events is the main method of capturing trace events
before component_manager has started.

### Enable the Kernel Tracing Boot Parameter

The size of the kernel's trace buffer can be changed at boot time
with the `ktrace.bufsize=N` command line option, where `N` is the size
of the buffer in megabytes.

The choice of data to collect is controlled with the `ktrace.grpmask=0xNNN'
command line option. The 0xNNN value is a bit mask of *KTRACE\_GRP\_\**
values from
//zircon/kernel/lib/boot-options/include/lib/boot-options/options.inc.
The default is 0x000, which disables all trace categories (or groups in
ktrace parlance).

The kernel command line arguments are changed locally in your build by
[setting local kernel options][kernel-options]

Example:

```gn
assembly_developer_overrides("custom_kernel_args") {
  kernel = {
    command_line_args = [ "ktrace.grpmask=0xfff", "ktrace.bufsize=32" ]
  }
}
```

You'll then need to rebuild and redeploy.

For more information on Zircon command line options see:
- [kernel_cmdline](/docs/reference/kernel/kernel_cmdline.md)
- [kernel_build](/docs/development/kernel/build.md)

### Including kernel boot trace data in trace results

Once you enable the kernel tracing boot parameter, as long as the kernel's internal trace buffer is
not rewound, after boot, the data is available to be included in the trace. This is achieved by
passing category `kernel:retain` to the `ffx trace` program. Note that the moment a trace is made
without passing `kernel:retain` then the ktrace buffer is rewound and the data is lost.

Example:

```posix-terminal
ffx trace start --categories "kernel:retain" --buffer-size 32 --duration 1
```

There are a few important things to note here.

The `kernel:retain` category tells `ktrace_provider` to read out the current contents of the trace
buffer instead of starting a new trace.

The second is the buffer size. The kernel's default trace buffer size is 32MB whereas the Fuchsia
trace default buffer size is 4MB. Using a larger Fuchsia trace buffer size means there is enough
space to hold the contents of the kernel's trace buffer.

The third important thing to note is that in this example we just want to grab the current contents
of the trace buffer, and aren't interested in tracing anything more. That is why a duration of one
second is used.

[kernel-options]: /docs/development/kernel/build.md#options
