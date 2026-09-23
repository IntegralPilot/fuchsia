---
name: verify-snapshot
description: >
    Takes and verifies a snapshot from a Fuchsia device to ensure file contents and system state
    are captured correctly. Use when modifying code that affects what goes into a snapshot
    (like //src/developer/forensics) or when you need to examine the state of the system via a
    snapshot.
---

# Verify Snapshot

When code changes affect what is captured in a Fuchsia snapshot (e.g., modifying components under
`//src/developer/forensics`), you should verify the changes by taking a snapshot from a running
device and examining its contents.

## Taking and Verifying a Snapshot

1. **Build Fuchsia image**: Run `fx build`. Do NOT forget this step.
2. **Launch the emulator**:
   Run `ffx emu stop; ffx emu start --net tap --headless`. This will stop any running emulators and
   start a new one with the latest build.
3. **Take the snapshot**:
   Run `ffx target snapshot`.
   - This command downloads a snapshot from the target and will print the path to the snapshot.
4. **Unpack the snapshot**:
   Important: Before running this step, wait for the previous step to output the path to the
   snapshot. Unzip the created snapshot into /tmp to examine its contents.
   - Example: `unzip -o /path_from_step_2/snapshot.zip -d /path_from_step_2/unzipped`
5. **Examine the contents**:
   Check the unpacked directory for the expected changes. The snapshot contains various files
   containing annotations, system logs, and inspect data.
   - Example: `grep -r "my_expected_string" /path_from_step_2/unzipped/annotations.json`

## Verification Checklist

Copy this checklist and track progress when a snapshot verification is required:
- [ ] Step 1: Build the system with your changes (`fx build`)
- [ ] Step 2: Ensure the device/emulator is running the new image (`ffx emu stop; ffx emu start --net tap --headless`)
- [ ] Step 3: Wait for the system to boot and generate relevant state
- [ ] Step 4: Run `ffx target snapshot`
- [ ] Step 5: Unzip the resulting snapshot archive
- [ ] Step 6: Verify the contents match the expected changes made in code
