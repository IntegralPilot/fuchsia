# Fake Battery CLI

`fake-battery-cli` is a command-line interface for developers to inspect and dynamically control the simulated battery and power source states on the `fake-battery` driver.

It connects to the driver's [`test.hardwarepowercontrol.Control`](/src/power/testing/fake-hardware-power-control/control.test.fidl) and [`fuchsia.hardware.power.battery.Battery`](/sdk/fidl/fuchsia.hardware.power.battery/battery.fidl) protocols exposed in the driver's `/out` directory.

---

## Usage

### 1. Build and Include

Ensure your build configuration includes the `fake-battery` driver and `fake_battery_cli_pkg`:

```bash
# Add to your fx set args or universe package list
fx set <product>.<arch> --with //src/power/testing/fake-battery/tool:package
fx build
```

### 2. Run via `ffx component explore`

Since the control FIDL is served directly in the driver's outgoing namespace, launch an interactive exploration shell inside the `fake-battery` driver component with the tool injected:

```bash
# Start an exploration shell in the fake-battery driver
[host]$ ffx component explore fake-battery --tools fuchsia-pkg://fuchsia.com/fake_battery_cli_pkg
Moniker: bootstrap/boot-drivers:dev.sys.platform.fake-battery
$ fake-battery-cli --help
```

---

## Subcommands and Examples

### `get` - Inspect Current State

Query and print current battery telemetry and power source status:

```bash
$ fake-battery-cli get
=== Fake Battery Status ===
  Level:                98.7%
  Charge Status:        Charging
  Health:               Good
  Temperature:          380 mC (0.4°C)
  Remaining Capacity:   382000 µAh
  Full Charge Capacity: 420000 µAh
  Time Remaining:       59 s
--- Power Source Status ---
  Present:              true
  Voltage:              4752000 µV (4.752 V)
  Current:              250014 µA (250.014 mA)
  Role:                 Sink (type: Ac, name: Fake AC Charger)
```

### `set` - Inject Simulated Telemetry

#### Change Battery Level and Status
```bash
# Set battery to 45% discharging
$ fake-battery-cli set --level 45.0 --status discharging --source none

# Set battery to 100% full on AC
$ fake-battery-cli set --level 100.0 --status full --source ac
```

#### Change Power Source and Electrical Metrics
```bash
# Simulate USB charging with 5V and 1.5A
$ fake-battery-cli set --source usb --status charging --voltage-mv 5000 --current-ua 1500000

# Simulate discharging under load at 3.7V and 800mA draw
$ fake-battery-cli set --source none --status discharging --voltage-mv 3700 --current-ua -800000
```

#### Change Battery Temperature and Health
```bash
# Simulate overheating condition (45.0°C)
$ fake-battery-cli set --temp-mc 45000 --health hot
```

---

## Non-Interactive Host Scripting

You can run automated commands directly from your host machine without entering the interactive shell using `ffx component explore -c`:

```bash
[host]$ ffx component explore fake-battery \
    --tools fuchsia-pkg://fuchsia.com/fake_battery_cli_pkg \
    -c "fake-battery-cli set --level 15.0 --source none --status discharging"
```

---

## Running Unit Tests

Run the unit tests with:

```bash
fx test fake-battery-cli-unittests
```
