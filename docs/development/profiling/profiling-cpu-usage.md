# Profiling CPU Usage with ffx profiler

`ffx profiler` is a tool that allows you to find and visualize
hotspots in your code. The CPU profiler periodically samples your running
threads and records backtraces, which can be viewed with the
[`pprof`](https://github.com/google/pprof){:.external} tool.

## When to use `ffx profiler`

The CPU profiler is best suited for identifying where CPU time is being spent
over a period. By taking frequent stack samples, the profiler builds a
statistical picture of execution, helping you find performance bottlenecks
("hotspots") without needing to modify or instrument your code.

- **Use `ffx profiler`** when you want to know _what functions_ are consuming
  the most CPU time across your system or within a specific component.
- **Use `ffx trace`** when you need to understand the _sequential flow_ of
  events, latency between specific operations, or interactions between different
  processes (e.g., IPC) over time. Tracing requires developers to add
  [trace events][trace-events] to the source code to be effective.
- **Use `zxdb` (Debugger)** when you need to inspect the exact execution state,
  step through code, or analyze memory at a specific point in time to
  understand correctness issues.

## Prerequisites and Setup

To get the most accurate profiles with the lowest observer overhead, always use
**Release builds** with the **kernel assisted thread sampler** enabled.

### The Importance of `--release` Builds

Always profile against a `--release` build. Debug builds lack inlining and
optimization passes. For languages like Rust and C++, the "zero-cost
abstractions" in their standard libraries are not zero-cost unless optimizations
are applied. Profiling a debug build will yield a profile dominated by internal
standard library calls (like iterator and `Option` handling) rather than your
actual application logic.

While release builds might make stacks slightly harder to follow due to
inlining, they provide an accurate representation of the actual performance.

## Common Use Cases and Examples

### System-Wide Profiling

To profile everything running on the device, including the root job and all of
its descendants, use the `--system-wide` flag:

```posix-terminal
ffx profiler attach --system-wide --duration 10
```

This will run for 10 seconds and generate a `profile.pb` file.

### Running and Profiling a Test

You can instruct the profiler to launch a test component and profile its
execution until it finishes. Note that the target package must be available in
your build graph (if it is a new or optional test, you may need to explicitly
add it with `fx with` or `fx add-test` and rebuild). Use the `--test` flag:

```posix-terminal
ffx profiler launch \
    --url "fuchsia-pkg://fuchsia.com/gtest_target#meta/gtest_target.cm" \
    --test
```

To profile specific test cases within that package, use `--test-filters`:

```posix-terminal
ffx profiler launch \
    --url "fuchsia-pkg://fuchsia.com/gtest_target#meta/gtest_target.cm" \
    --test \
    --test-filters "GtestTest.MakeWorkTest"
```

### Background Profiling (Disconnecting from the Host)

Sometimes you need to profile events where the connection to the host machine
might drop, such as across a **Suspend/Resume** cycle. For this, run the
profiler session in the background.

1. Start the profile in the background:

   ```posix-terminal
   ffx profiler attach --system-wide --background
   ```

   The CLI outputs a confirmation such as:

   ```none {:.devsite-disable-click-to-copy}
   Background session started. task_id: 1
   ```

   Your device is now continuously recording profile samples in the background.

2. Disconnect your host, trigger a suspend, or perform the actions you wish to
   measure. Wait for the device to wake up and reconnect to the host.

3. Stop the session and download the profile. This command will find the running
   background session, stop it, and download the data to the host:

   ```posix-terminal
   ffx profiler stop
   ```

   ```none {:.devsite-disable-click-to-copy}
   Wrote profile to profile.pb
   ```

### Profiling on boot

You can configure the profiler to attach to processes as soon as they start upon
the next device reboot. This is useful for analyzing early startup performance
or the boot sequence.

1. Start the profile session targeting the next boot:

   ```posix-terminal
   ffx profiler attach --system-wide --on-boot
   ```

   The CLI will output a confirmation such as:

   ```none {:.devsite-disable-click-to-copy}
   On-boot profiling session configured. Profiles will be collected when the
   target component starts.
   ```

2. Reboot your device.

3. Wait for the device to finish booting and the target processes to start. You
   can check the status of active sessions using the `status` command.

4. Stop the session to download the accumulated profile:

   ```posix-terminal
   ffx profiler stop
   ```

### Checking profiler status

To view any active background or on-boot profiling sessions that are currently
running on the device, use the `status` command:

```posix-terminal
ffx profiler status
```

```none {:.devsite-disable-click-to-copy}
Active background sessions:
  - task_id: 1
```

### Attaching to Existing Processes

You can attach to specific components or processes.

**By Component URL or Moniker:**

First, find the moniker of your target component. You can list all components
currently running on the system using `ffx component list`:

```posix-terminal
ffx component list
```

```none {:.devsite-disable-click-to-copy}
.
bootstrap
bootstrap/archivist
bootstrap/archivist/archivist-pipelines
...
core/your_component
```

Then attach using the resulting moniker:

```posix-terminal
ffx profiler attach --moniker core/your_component
```

Alternatively, you can attach using the component's package URL:

```posix-terminal
ffx profiler attach --url 'fuchsia-pkg://fuchsia.com/your_component#meta/your_component.cm'
```

**By KOIDs (PIDs/TIDs/Job IDs):**
First, find the Process ID (PID) of your target taking advantage of the `ps`
command on the device:

```posix-terminal
ffx target ssh ps
```

```none {:.devsite-disable-click-to-copy}
TASK                       PSS PRIVATE  SHARED   STATE NAME
j: 1045                 620.7M  464.7M                 root
  p: 1122                17.7M   17.7M   4944K         bin/component_manager
...
    j: 1944             126.0K     24K
      p: 1993           126.0K     24K   5076K         kernel-args-forwarder.cm
```

Then attach to the resulting PID (in this example, `1993` for `kernel-args-
forwarder.cm`):

```posix-terminal
ffx profiler attach --pids 1993 --duration 5
```

_(Specifying a PID automatically profiles all threads within that process)._

## Command line parameters & best practices

The following options are common to `ffx profiler attach`, `ffx profiler
launch`, and `ffx profiler stop`:

- `--output`: Name or path of the output trace file. Defaults to `profile.pb`.
- `--print-stats`: Print stats about how the profiling session went to stdout.
- `--color-output`: If true, include color codes in output. Defaults to true if
  terminal output is detected.

The following options apply to both `ffx profiler attach` and `ffx profiler
launch`:

- **Sample Period (`--sample-period-us`)**: The default sample period is
  `10000` microseconds (10 ms). Decreasing this value (e.g., to 1 ms) provides
  higher resolution but increases the CPU overhead of the profiler itself,
  potentially altering the system behavior you are trying to measure
  (the "observer effect").
- **Buffer Size (`--buffer-size-mb`)**: If you are profiling a highly active
  system over a long duration, you may exhaust the default buffer size before
  the profiler finishes capturing. If you encounter missing samples or warnings,
  increase the memory allocation.
- **Duration (`--duration`)**: If `--duration` is unspecified, the profiler
  will run interactively and wait until you press `<ENTER>` to stop capturing.
- **Background (`--background`)**: Run the profiler session in the background.

The following option applies only to `ffx profiler attach`:

- **On Boot (`--on-boot`)**: Run the profiler session when the device next boots.

There are additional options available. These are primarilary used to
troubleshoot the profiler and provide fine-grained control of the profiler
execution.
See the [ffx profiler reference][ffx-profiler] for more details.

## Analyzing the Profile

Once the profiler stops, it generates a `profile.pb` file in your current
directory. You can analyze this file using the Perfetto UI or Google's
[`pprof`](https://github.com/google/pprof) tool.

### Perfetto UI (Recommended)

You can upload the `profile.pb` directly to the [Perfetto UI](https://ui.perfetto.dev/)
to visualize the profile in your browser. This is often the most intuitive
and feature-rich way to explore the profile.

### Interactive web UI with `pprof`

You can also use the interactive web interface of `pprof`, which
includes a graphical Flame Graph.

```posix-terminal
pprof -http=localhost:8080 profile.pb
```

Your browser will open to `http://localhost:8080`. From the top menu, you can
select views such as:

- **Top**: Shows the functions consuming the most flat CPU time.
- **Flame Graph**: Visually represents the call stack hierarchy. The width of
  a box indicates the total time that function or its children were sampled.

### Terminal Top Functions

To quickly print the top functions directly to your terminal:

```posix-terminal
pprof -top profile.pb
```

This command produces text output showing flat and cumulative percentages:

```none {:.devsite-disable-click-to-copy}
Showing nodes accounting for 272, 100% of 272 total
      flat  flat%   sum%        cum   cum%
       243 89.34% 89.34%        243 89.34%   count(int)
        17  6.25% 95.59%        157 57.72%   main()
         4  1.47% 97.06%          4  1.47%   collatz(uint64_t*)
         3  1.10% 98.16%          3  1.10%   add(uint64_t*)
         3  1.10% 99.26%          3  1.10%   sub(uint64_t*)
         1  0.37% 99.63%          1  0.37%   rand()
```

- **flat**: Number of samples where this function was actively executing at the top of the stack.
- **cum**: Number of samples where this function was executing _or_ any of its descendants were executing.

[trace-events]: /docs/development/tracing/trace_events.md
[ffx-profiler]: /reference/tools/sdk/ffx#ffx_profiler
