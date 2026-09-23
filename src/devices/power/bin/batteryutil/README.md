# batteryutil

`batteryutil` is a command-line diagnostic and control utility for battery fuel gauges and power
path chargers on Fuchsia.

It connects to `fuchsia.hardware.power.battery.Service` (Fuel Gauge data plane),
`fuchsia.hardware.power.charger.Service` (Charger enable control), and `fuchsia.power.battery`
services.

## Specifying Paths & Multiple Devices

By default, `batteryutil` automatically discovers available battery and charger service instances
under `/svc/`. If multiple instances are present:
- `get` queries and reports telemetry for all discovered instances.
- `watch` multiplexes real-time streaming updates across all discovered instances.
- `enable` selects an available charger instance, prompting or warning if multiple are present.

To target a specific battery or charger instance directly, pass the `-p` / `--path` option before
the command:

```bash
# Query a specific battery service instance
$ batteryutil -p /svc/fuchsia.hardware.power.battery.Service/default get

# Watch a specific fuel gauge instance
$ batteryutil -p /svc/fuchsia.hardware.power.battery.Service/default watch

# Enable or disable a specific charger instance
$ batteryutil -p /svc/fuchsia.hardware.power.charger.Service/default enable 1
```

## Commands

### 1. Telemetry Inspection (`get`)
Query real-time battery hardware status (SOC, voltage, current, temp, cycles).

```console
$ batteryutil get
Model: MAX77779
Chemistry: Li-Ion
Design Capacity: 4.947 Ah
Design Voltage: 3.850 V
Supported Triggers: level_percent, charge_status, cycle_count
Supported Wake Triggers: None
Present: true
Charge Status: Charging
Level: 26.1%
Remaining Capacity: 1.295 Ah
Full Charge Capacity: 4.947 Ah
Temperature: 30.5 C
Voltage: 3.897 V
Current: 1.251 A
Cycle Count: 2
Time Remaining: 20m 44s (1244.6s)
```

### 2. Real-Time Event Streaming (`watch`)
Stream state transitions and telemetry changes via hanging-get without polling.

```console
$ batteryutil watch
Watching battery events on all instances (press Ctrl+C to exit)...

=== Battery Telemetry Update (/svc/fuchsia.hardware.power.battery.Service/default) ===
Present: true
Charge Status: Charging
Level: 26.1%
Remaining Capacity: 1.295 Ah
Full Charge Capacity: 4.947 Ah
Temperature: 30.5 C
Voltage: 3.897 V
Current: 1.251 A
Cycle Count: 2
Time Remaining: 20m 44s (1244.6s)

=== Battery Telemetry Update (/svc/fuchsia.hardware.power.battery.Service/default) ===
Present: true
Charge Status: Charging
Level: 27.0%
Remaining Capacity: 1.335 Ah
Full Charge Capacity: 4.947 Ah
Temperature: 30.6 C
Voltage: 3.912 V
Current: 1.248 A
Cycle Count: 2
Time Remaining: 20m 05s (1205.2s)
```

### 3. Charger Enable/Disable (`enable`)
Enable or disable battery charging:

```console
$ batteryutil enable 1
Successfully enabled charging via fuchsia.hardware.power.charger.Charger (/svc/fuchsia.hardware.power.charger.Service/default/charger)

$ batteryutil enable 0
Successfully disabled charging via fuchsia.hardware.power.charger.Charger (/svc/fuchsia.hardware.power.charger.Service/default/charger)
```

### 4. Low-Level Power Source Override (`power`) — *Sorrel Only*
Low-level Qualcomm SPMI debug register override (`0x2954`) for manual bench testing on **Sorrel**
boards only:

```console
$ batteryutil power battery
Successfully set power source to battery via SPMI (wrote 0x01 to 0x2954)

$ batteryutil power usb
Successfully set power source to usb via SPMI (wrote 0x00 to 0x2954)
```

