---
name: manage-emulator
description: Instructions for starting, stopping, and checking the Fuchsia emulator.
---

# Manage Emulator Skill

This skill provides instructions for managing the Fuchsia emulator.

## Workflow

### 1. Check Emulator Status
To see if an emulator is running and detected:
`ffx target list`

Look for a target with a name like `fuchsia-emulator` or a similar name indicating it is an
emulator.

### 2. Start Emulator
If no emulator is running, start one in headless mode:
`ffx emu start --headless`

### 3. Restart Emulator
If an emulator is running and you need to restart it (e.g., after a build):
1. Stop the emulator:
   `ffx emu stop`
2. Start the emulator again:
   `ffx emu start --headless`

After starting, schedule a timer for 30 seconds to check status using `ffx target list`.
If it is not ready, check again in another 30 seconds.
