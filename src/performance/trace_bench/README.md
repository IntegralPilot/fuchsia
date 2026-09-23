# Tracing Benchmarks

This suite has two sets of benchmarks:

A throughput benchmark set up a trace provider, a trace reading component, and trace_manager and
then attempts to read/write as much data to trace manager as possible. It then measures the time to
transfer 1GiB of trace data in nanoseconds.

A configuration benchmark set up 20 trace providers, a trace reading component, and trace_manager and
then attempts to init/start/stop/terminate many times.

## Running

Add the benchmark to your package set
```
fx set ... --with //src/performance/trace_bench:tests
```
Run `fx serve` in the background and run the benchmark component
```
fx test trace_system_thoughput_benchmarks_standalone --output  -- -p --runs 10
# or
fx test trace_system_configuration_benchmarks_standalone --output  -- -p --runs 10
```

You should then get output along the lines of:
```
      Mean    Std dev        Min        Max     Median Unit         Test case
   1215142     587086     546099    3280213    1023905 nanoseconds  StreamingModeThroughput/1GiB.setup
 676671216   74771251  594963484  864167716  657734789 nanoseconds  StreamingModeThroughput/1GiB.read_data
  24534160    8801850    6375685   39983547   24721210 nanoseconds  StreamingModeThroughput/1GiB.cleanup

```

or if you ran the configuration benchmarks:

```
      Mean    Std dev        Min        Max     Median Unit         Test case
    357843     470936      57681    1966946     141966 nanoseconds  Configure/20Providers/32MBBuffers.setup
     44338      28859       9718     151750      35682 nanoseconds  Configure/20Providers/32MBBuffers.init_tracing
   5618729    1556973    2943349    8546837    5434496 nanoseconds  Configure/20Providers/32MBBuffers.start_tracing
   4331597    1263093    2158010    6385332    4333479 nanoseconds  Configure/20Providers/32MBBuffers.stop_tracing
   4900415    1008924    2657100    6817254    4930504 nanoseconds  Configure/20Providers/32MBBuffers.terminate_tracing
    225501     401797      15788    1598713      22473 nanoseconds  Configure/20Providers/32MBBuffers.cleanup
    297855     337682      57283    1184771     188696 nanoseconds  Configure/20Providers/64MBBuffers.setup
    207134     389738      33844    1566286      36984 nanoseconds  Configure/20Providers/64MBBuffers.init_tracing
   6064789    4198961    1986528   22905596    5376452 nanoseconds  Configure/20Providers/64MBBuffers.start_tracing
   4549663    1045054    2797488    7673153    4289523 nanoseconds  Configure/20Providers/64MBBuffers.stop_tracing
   4787850    1301652    1972981    7437368    4765726 nanoseconds  Configure/20Providers/64MBBuffers.terminate_tracing
    376561     569026      16347    1771054      25077 nanoseconds  Configure/20Providers/64MBBuffers.cleanup
    620914     993569      58491    3974096     161613 nanoseconds  Configure/20Providers/8MBBuffers.setup
     64338      51214      10204     209562      38118 nanoseconds  Configure/20Providers/8MBBuffers.init_tracing
   5787589    1509081    2949313    8444341    5347576 nanoseconds  Configure/20Providers/8MBBuffers.start_tracing
   3934088    1182910    2034066    7026912    4079228 nanoseconds  Configure/20Providers/8MBBuffers.stop_tracing
   6248422    3784572    2781467   17891231    5272355 nanoseconds  Configure/20Providers/8MBBuffers.terminate_tracing
    164461     150798      15646     407526     117774 nanoseconds  Configure/20Providers/8MBBuffers.cleanup
```

