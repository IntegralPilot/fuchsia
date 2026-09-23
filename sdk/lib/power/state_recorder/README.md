# State reporting

This directory provides Rust and C++ libraries that support standardized
reporting of time series data via Inspect and trace. It supports recording of
**enum states** and **numeric states**.

Currently, both the Rust and C++ APIs support enum and numeric states. The Rust
API also provides an option to persist the states across component crashes and
device reboots (populating Inspect nodes under `previous_boot_history`).
There is no persistence option yet for the C++ API.

Enum states generally correspond to categorical observations. They are
well-suited for scenarios in which the name of the state, rather than an
underlying numeric value, is most relevant for analysis purposes. Examples
include:
* A device with on and off states.
* A sensor with high-power, low-power, and off states.
* The charging state of a battery-powered device: discharging, charging, or
  fully-charged.

Numeric states can describe anything numeric-valued, including:
* System-configured states, like clock frequencies, that are precisely known
  and may be reported at every transition.
* Estimated values of system properties, like CPU load and battery level.

# Examples

Examples in both Rust and C++ are located at
[`//examples/power/state_recorder`][strc].

When updating the libraries, these examples should be used to confirm that trace
output behaves as expected, as that depends on visual expectations in the
Perfetto UI for which we don't have automated tests.

## Output formats

### Stability

The formats presented here may evolve based on emerging needs. If you are aware
of a stakeholder that may not be under consideration, please contact a code
owner!

### Inspect

Inspect data for all recorded states will be placed in a node named
`power_observability_state_recorders` that is a child of the inspector's root.
Each recorded state will be given a distinct node.

The key elements of the data are:
* The name of the entity. Both the C++ and Rust APIs guard against name
      collisions.
* The state type, enum or numeric.
* For enum states: The mapping between state names and integer representations.
* For numeric states:
    * Units of values.
    * (Optional) The range of expected values. Both bounds are inclusive. *The
      range is for eventual use by tooling (e.g. to select plot bounds) and
      does not affect the API (no errors for recording out-of-range values).
      If supporting exclusive bounds or API support for out-of-range values
      would be helpful to you, please [file a
      bug](https://issues.fuchsia.dev/issues?q=componentid:1585130).*
* History of state values: Stored under a `history` (or `previous_boot_history`)
  node containing `current_index`, `current_size`, and a `shards` child node
  with numeric array properties (`times` and `values`).

### Trace

Enum states are recorded as slices with state names, with each recorded entity
receiving a unique track. Numeric states are recorded as counters.

### Sample output

#### Inspect

Below are examples involving a battery, with charge recorded as an integer
percentage once per minute, and charging state -- one of `Charging`,
`FullyCharged`, or `Discharging` -- recorded on transition.

The specifications in the table result in the Inspect data that follows:

| Time (sec) | Event |
|------------|-------|
| 0          | Battery at 98% charge, and `Charging`; charges 1% per minute |
| 120        | Charge increases to 100%; now `FullyCharged` |
| 240        | Battery is `Discharging`; drains 1% per minute |

```
    root:
      power_observability_state_recorders:
        battery_level:
          metadata:
            format_version = 2.0
            name = battery_level
            range:
              min_inc = 0
              max_inc = 100
            type = numeric
            units = percent
          history:
            current_index = 8
            current_size = 8
            shards:
              0:
                times = [0, 60000000000, 120000000000, 180000000000, 240000000000, 300000000000, 360000000000, 420000000000]
                values = [98, 99, 100, 100, 100, 100, 99, 98]
          previous_boot_history:
            current_index = 0
            current_size = 2
            shards:
              0:
                times = [8000000000, 110000000000]
                values = [98, 99]
        charging_state:
          metadata:
            format_version = 2.0
            name = charging_state
            type = enum
            states:
              Charging = 1
              Discharging = 0
              FullyCharged = 2
          history:
            current_index = 3
            current_size = 3
            shards:
              0:
                times = [0, 120000000000, 240000000000]
                values = [1, 2, 0]
```

##### Interpreting the Circular Buffer
To reconstruct the chronological sequence of state samples:

1. **Reconstructing Chronological Order**:
   - `current_size`: Total number of valid elements currently recorded, up to
     `capacity`.
   - `current_index`: The next insertion index (head) in the buffer.
   - **Before the buffer wraps (`current_size < capacity`)**:
     Elements are located at indices `0` through `current_size - 1` in
     chronological order.
   - **After the buffer wraps (`current_size == capacity`)**:
     `current_index` points to the oldest sample (which will be overwritten
     next). The logical sample `i` (from oldest `0` to newest
     `current_size - 1`) is located at circular index
     `(current_index + i) % capacity`.
   - Within `shards`, index `k` maps to shard `k / 200` at slot `k % 200`.

2. **Interpreting Values**:
   - **Numeric States**: Values are stored as array properties (`times` and
     `values`).
   - **Enum States**: Integer values in `values` are translated back to state
     names using the reverse mapping of `metadata.states`.

#### Trace

To demonstrate trace output, we modify the Inspect example to span a larger
range of battery levels and key behavior on abstract "ticks":

| Time (ticks) | Event |
|------------|-------|
| 0          | Battery at 90% charge, and `Charging`; charges 1% per tick |
| 10         | Charge increases to 100%; now `FullyCharged` |
| 15         | Battery is `Discharging`; drains 1% per tick |

![Sample trace output showing charging state and battery level](trace_example.png)


[strc]: https://cs.opensource.google/fuchsia/fuchsia/+/main:examples/power/state_recorder

