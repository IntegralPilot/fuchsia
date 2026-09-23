---
name: wlan-run-e2e-test
description: >
  Workflow for running Fuchsia WLAN E2E tests on local devices or remote infrastructure, verifying testbed hardware requirements, monitoring execution, analyzing failure output, configuring testing product bundles, and troubleshooting physical topology routing.
---

# Fuchsia WLAN E2E Test Workflow

This skill encapsulates the end-to-end workflow for running and analyzing Fuchsia WLAN E2E tests against both local devices (including physically complex testbeds) and remote infrastructure builders.

## 1. Running E2E Tests Locally

When conducting tests on a local target, execute the following steps:

### 1.1 Verify Target Device Presence
Before initiating the test suite, ensure the Device Under Test (DUT) is connected and responsive:
*   **Command:** `ffx target list` to discover it.
*   **Command:** `ffx target echo` to verify responsiveness.
*   **Note:** The device may be physically attached OR it may be remote-forwarded using tools like `mrt` or `pontis`. If you hit an issue with device communication, proactively ask the user whether the device is remote or locally forwarded, as this often affects reachability. Ensure the user verifies that `mrt` or `pontis` is up and working.

### 1.2 Verify Test Hardware Requirements
WLAN E2E tests frequently require specialized hardware like Access Points (Whirlwind, OpenWRT), PDUs (DigitalLoggers, Synaccess), software-controlled attenuators, etc.
1.  **Analyze Test Code:** Check the relevant Python test file to see what specific hardware arguments are required (e.g., `ap-ip`, `ap-ssh-port`). Common locations include:
    *   `//src/testing/end_to_end/antlion/tests/`
    *   `//src/connectivity/wlan/tests/`
2.  **Verify Presence:** Look in the user's prompt or local environment for evidence that the needed equipment is present. This hardware can be local or forwarded via SSH reverse tunnels (e.g. `ssh -R 8000:<AP_IP>:22 <REMOTE_HOST>`) or `mrt`.
3.  **Fallback to Execution:** If you are ultimately unable to determine the presence or absence of the required test equipment from the prompt or environment, **proceed to run the test**. If it errors out or fails, analyze the output (e.g., connection timed out to AP IP) to determine if missing or misconfigured hardware was the cause.

### 1.3 Run the Fuchsia E2E Test
Launch tests using `fx test`. E2E tests require specific flags followed by a `--` separator to pass the hardware configuration parameters to the test script.
*   **Example Syntax:** `fx test --output --timeout 1200 --e2e sched_scan_test -- --ap-ip 127.0.0.1 --ap-ssh-port 8023 --device "[::1]:8022"`
*   **Pattern:** `fx test --output --timeout 1200 --e2e <test_name> -- <equipment_params...>`

**Key E2E Testing Caveats & Requirements:**
*   **Package Server (`fx serve`):** Some tests actively pull packages and require a package server to be running. If a package server is not running, the `fx test` command should automatically raise one by default. However, if it does not or you observe an error that mentions a package server, you may run `fx serve` in parallel as a background task. **If you manually started a package server in a background task, make sure to kill that task when you're done, otherwise it will linger forever.**
*   **Targeting the Device (`--device`):** Relying entirely on FFX's mDNS discovery or standard default targets might cause E2E tests to hang or fail to parse local ports correctly. Instead, pass the test runner the device target explicitly under the E2E parameter block by setting `--device <device_name>`.
*   **Test Timeouts:** Long E2E testing orchestrations may surpass `fx test`'s default 5-minute timeout. You should proactively add `--timeout <seconds>` to extend the failure boundary.
*   **Filtering E2E Test Cases:** The standard `fx test --test-filter <test>` flag **does not work** for Python E2E tests because the framework ignores Fuchsia's standard test-filter environment variables. Instead, to run an isolated test case (e.g., `test_simultaneous_pings`), simply append the name of the individual test cases as positional arguments to the *very end* of the command, after the `--` block:
    `fx test ... --e2e ping_test -- <args> test_simultaneous_pings`

### 1.4 Monitor the Test Execution Task
Test times vary wildly. They should continually print progress to stdout. Keep an eye on the background task for indefinite hangs with no log progression-if the test is frozen without output, you may need to kill the process and investigate.

### 1.5 Analyze Test Output
Once execution is complete:
1.  **Review the Summary:** Parse the final summary block showing numbers of tests passed, failed, skipped, and errored.
2.  **Analyze Failures:** Scan the `stdout` logs for panics, assertion failures, and Python stack traces.
3.  **Report to User:** Always start your response with a **concise overview** of the test run's status. Follow this immediately with a **short analysis** explaining why any failures or errors occurred.

---

## 2. Running Tests with `fx test-remote`

When targeting tests for which the user does not have the necessary local hardware (e.g. attenuator), utilize remote infrastructure.

### 2.1 Use the `fx-test-remote` Skill
Apply the procedures defined in the [fx-test-remote](//.agents/skills/fx-test-remote/SKILL.md) skill to execute tasks on infrastructure. Look for WLAN specific builders.

### 2.2 Configure `fx set` (User Confirmation Required!)
Testing remotely often demands reconfiguring the local Fuchsia checkout to match the remote builder's parameters.
*   **Actionable Rule:** You may need to run `fx set` to align the build graph. If you suspect this is required, propose the specific `fx set` command immediately in your response to the user so they can approve or deny it without blocking.
*   ** CRITICAL:** NEVER run an `fx set` command without the user's explicit confirmation, as doing so will wipe their existing build outputs.

### 2.3 Analyze Results
After kicking off an infra run, use infra tools (`bb` or `logdog` as documented in `fx-test-remote`) to retrieve and analyze test logs. Follow `wlan-luci-triage` principles to parse snapshots and test framework errors.

---

## 3. Additional Features & Capabilities (Expert Troubleshooting)

When assisting users with WLAN E2E tests, proactively utilize your knowledge of Fuchsia WLAN test topologies to provide expert assistance:

### 3.1 Resolving Broken Testbed Infrastructure
If tests fail to communicate with equipment (DUTs, APs, PDUs), assist with network debugging:
*   **Ask About Forwarding:** Consistently inquire whether the device is remote or locally forwarded if you hit connectivity issues. If the device is remote, remind the user to double check their remote forwarding tools, including `mrt`, `pontis`, etc.
*   **SSH Access Point Rejections:** If SSH to a Whirlwind or OpenWRT AP fails (e.g. "Connection timed out during banner exchange"), advise the user to edit their `~/.ssh/config` to use `ProxyCommand none` for the AP's subnet to bypass corp SSH relays.
*   **Unreliable mDNS/Target Discovery:** If the host is failing to consistently discover the Fuchsia device or if tunnels repeatedly drop, suggest verifying their device is discoverable via `ffx target list`, responds to `ffx target echo`, and reachable via `ffx target ssh`.

### 3.2 Recommending Build Contexts and Product Bundles
WLAN tests run against specific testing product bundles which exclude default production components (like `cast_agent`) that could interfere with the tests.
*   **Build Setup:** Remind users configuring their local development setup for specific tests that they may need to include: `--args='product_bundle_labels+=["//vendor/google/tests/end_to_end/wlan/product_bundles"]'` to access the relevant product bundles.
*   **Matching Tests to Bundles:** Determine the correct product bundle for your test based on the directory that contains the test, where `<board name>` is `astro`, `nelson`, `sherlock`, or `sorrel`. This list is not exhaustive, so consider asking the user for confirmation:
    *   `//src/testing/end_to_end/antlion` -> `for-testing-wlan-platform.<board name>`
    *   `//src/connectivity/wlan/tests/core` -> `for-testing-wlan-core.<board name>`
    *   `//src/connectivity/wlan/tests/wlancfg` -> `for-testing-wlan-platform.<board name>`
    *   `//src/connectivity/wlan/tests/wlanix` -> `for-testing-wlan-wlanix.<board name>`
*   **Setting the Product Bundle:** Consider using `fx set-main-pb` to establish the primary product bundle logically instead of simply targeting it directly via `fx build <product-bundle>`.

## 4. Iterative Development & Test-Driven Development (TDD)

When actively developing WLAN features alongside writing or modifying E2E tests, it is critical to understand when to re-flash devices versus when merely re-running the test framework is sufficient.

### 4.1 Test Code vs. System Source Code
*   **Test Code Changes:** Changes exclusively to the Python E2E test files do NOT require flashing or rebooting the device. Simply rebuild (e.g. `fx build`) and re-run the test command (`fx test ...`). The Antlion runner will execute the updated test logic against the existing device state.
*   **Source Code Changes:** Changes to Fuchsia source code, WLAN drivers, or components in the WLAN stack MUST be deployed to the device before re-running tests. Building the code locally does not automatically update the device.

### 4.2 Deployment Strategies
Deployment tooling choices are predominately tied to the *size of the change* and the *target board*, rather than strictly the type of product bundle (e.g., testing vs development bundles). Both testing and development bundles support OTA and flashing.

*   **Incremental Updates (All Boards/Bundles)**
    When making iterative changes to served packages, use the OTA flow to rapidly delta-update without wiping device state.
    1.  Push the updated packages over the network using `fx force-ota-from-devhost` (or doing a manual OTA via `fx ota` / `ota-from-dev-host`). Note: this performs an interactive update avoiding a full re-flash, but it explicitly requires a package server, is not always as reliable as a flash, limits updates for certain built-in base components, and inherits other caveats.
    2.  Wait for the device to cycle and reconnect: `while true; do .jiri_root/bin/ffx target echo && break; sleep 1; done`

*   **Full Flashes and Major Changes**
    When deploying new product bundles, dealing with base components, or making structural changes, a full flash is almost certainly required. The specific flash command depends heavily on the board:
    1.  Reboot the device into the bootloader: `.jiri_root/bin/ffx target reboot -b`
    2.  Flash the device using the appropriate tooling:
        *   For `sorrel` boards, `fx flash-kola` is the primary and preferred tool over `fx flash`.
        *   For other generic boards, use `fx flash` (or board-specific tools like `fx flash-nelson`). NOTE: Flashing certain devices over remote connections often fails or hangs because `fx flash` cannot find the DUT in fastboot. Ensure reachability in fastboot using `pontis` (if that is the supported method) before assuming a device is truly unresponsive.
    3.  Poll until the device reconnects: `while true; do .jiri_root/bin/ffx target echo && break; sleep 1; done`

CRITICAL: Consider polling the user to determine which flash tool is preferable given their local hardware and the build type. Remind the user to ensure their devices are
connected, discoverable (e.g. `ffx target list`), and reachable (e.g. `ffx target ssh`) before proceeding. This will help ensure any user-specific setup (e.g. devices plugged in, remote SSH tunnels/forwarding, etc.) are prepared beforehand, and reduces the risk of getting stuck with unreachable devices.

### 4.3 Verifying Code Deployment (Revision Tagging)
In complex systems with many moving components, it can be difficult to verify if your newly compiled code successfully deployed and was loaded by the device.
**Helpful Practice:** Add a temporary, highly visible syslog statement with an incrementing "Revision" number to the initialization block of the class or function you are modifying.
Example: `LOGF(INFO, "MyComponent::Initialize [Revision 12] started");`
Always bump this revision number with every build and flash iteration. Before investigating a new test failure, immediately grep the test logs or `ffx log dump` for your revision tag (`[Revision 12]`) to confidently confirm your code is actively running on the device. Remove this logline when work is completed.

## 5. Other Compatible Skills
This skill is part of a larger ecosystem of testing workflows. Actively consult these related skills when relevant:
*   Global Fuchsia skills under `fuchsia/.agents/skills/`
*   WLAN domain-specific skills under `fuchsia/src/connectivity/wlan/.agents/skills/` (e.g., `wlan-luci-triage` to analyze remote LUCI CI failures).
