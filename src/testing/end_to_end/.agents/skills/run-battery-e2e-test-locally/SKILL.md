---
name: run-battery-e2e-test-locally
description: >
  Instructions for configuring, building, and running the Honeydew Battery
  End-to-End (E2E) test locally on a physical or emulated Fuchsia device.
---

# Running Battery End-to-End Tests Locally

This skill outlines how to configure the build, compile, and execute the Battery End-to-End (E2E) Mobly test (`//src/tests/end_to_end/battery:battery_test_fc`) against a local Fuchsia target (physical device or emulator).

---

## 1. Prerequisites & Target Setup

1. **Verify Target Connectivity**:
   Confirm that your target Fuchsia device or emulator is detected by `ffx`:
   ```bash
   ffx target list
   ```
   Ensure the target is reachable:
   ```bash
   ffx target echo
   ```

2. **Starting an Emulator (if no physical device is connected)**:
   ```bash
   ffx emu start --headless
   ```

---

## 2. Step-by-Step Workflow

### Step 1: Configure the Build
Configure the build with `fx set` to include the battery E2E test host bundle.

For x64 targets (or emulator):
```bash
fx set core.x64 --with-host //src/tests/end_to_end/battery:e2e_battery_test
```

For ARM64 targets (e.g. VIM3):
```bash
fx set core.arm64 --with-host //src/tests/end_to_end/battery:e2e_battery_test
```

> [!TIP]
> If you have already set your product/board configuration, you can append the test without reconfiguring from scratch:
> ```bash
> fx add-host-test //src/tests/end_to_end/battery:e2e_battery_test
> ```

---

### Step 2: Build the Artifacts
Compile the host test runner, Honeydew libraries, and target packages:
```bash
fx build
```

---

### Step 3: Run the Test
Execute the test using `fx test`:
```bash
fx test //src/tests/end_to_end/battery:battery_test_fc --e2e --output
```

#### Key Flags:
* `--e2e`: Designates the test as an end-to-end host test requiring an active Fuchsia device.
* `--output`: Streams test execution and Mobly logs directly to stdout.
* `-t <target_name>`: (Optional) Specify the target device name if multiple devices are running (e.g., `fx -t fuchsia-emulator test //src/tests/end_to_end/battery:battery_test_fc --e2e --output`).

---

## 3. Troubleshooting & Diagnostics

* **Device Capability Verification**:
  The battery test requires the `fuchsia.hardware.power.battery.Service` capability. To verify if it is routed on the device:
  ```bash
  ffx component capability fuchsia.hardware.power.battery.Service
  ```

* **Device Logs**:
  If the test fails during `get_status()`, dump the device-side system logs to inspect driver output:
  ```bash
  ffx log dump --severity DEBUG
  ```

* **Inspect Device Power Topology**:
  ```bash
  ffx inspect show "bootstrap/power-manager"
  ```
