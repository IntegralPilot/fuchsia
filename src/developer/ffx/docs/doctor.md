<!-- Copyright 2026 The Fuchsia Authors. All rights reserved. -->
<!-- Use of this source code is governed by a BSD-style license that can be -->
<!-- found in the LICENSE file. -->

# FFX Doctor User Diagnostic Guide

The `ffx doctor` subcommand is a comprehensive diagnostic utility designed to inspect, validate, and troubleshoot the local Fuchsia host development environment and its communication channels with target devices or virtual emulators.

If you experience communication hiccups or target discovery failures, running `ffx doctor` is the recommended initial triage workflow step.

## Usage Syntax

To run the standard environment diagnostic checklist pass, execute:

```posix-terminal
ffx doctor
```

### Common Command-Line Options

*   `-v, --verbose`: Enables granular verbose reporting, printing all hidden informational blocks (such as full API levels and ABI revisions details).
*   `--record`: Captures diagnostic logs and doctor output into a bundle for bug reports and debugging environment issues.

## What FFX Doctor Checks

The diagnostic checklist sequentially evaluates the following environment layers:

### 1. Host Toolchain and Environment Context
*   **Path to ffx**: Displays the absolute filesystem path of the active execution binary.
*   **Version Integrity**: Reports the frontend build compilation version tag.
*   **Environment Kind**: Validates whether the tool is operating inside a default user context or a specialized isolated sandbox root environment.
*   **Config Lock Health**: Scans and confirms that internal configuration database locks are healthy and un-stalled.
*   **Ssh Key Consistency**: Checks that the public and private Fuchsia SSH keys are present and structurally consistent.

### 2. Physical and Virtual Targets Environment
*   **FFX Emulator Instances**: Audits local active virtualization setups, listing running emulators or reporting empty slots.
*   **Inotify Watches Bounds**: (Linux only) Checks filesystem watcher limits to ensure the kernel has sufficient allocation handles to trace file changes during large build operations.
*   **FFX USB Driver Sockets**: Validates the operational status of the FFX USB host driver socket binds, ensuring they listen on the expected path locations.

### 3. Network and Target Verification Checks
*   **Target Discovery Loop**: Scans the local network map and prints the aggregate number of discovered target devices.
*   **Target Communication and RCS Health**: Performs detailed protocol binding checks against individual targets, reporting compatibility state parameters.

## Reading Outcomes Indicators

Every checklist node prints an explicit status symbol:
*   `[✓]`: **Success**. The parameter conforms to system expectations.
*   `[i]`: **Informational**. Neutral advisory data points (e.g., active binary paths, version tags).
*   `[!]`: **Warning**. Soft advisory anomalies encountered (e.g., unconfigured features, minor socket paths mismatches) that do not instantly break basic workflows.
*   `[✗]`: **Failure**. Critical configuration block faults detected that require immediate human manual remediation.
