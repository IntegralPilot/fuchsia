# Automated Bisection Scripts

This directory contains a collection of scripts that can be used in conjunction
with the `ffx product-bundle bisect` command to automate the bisection process.

## Automating Bisection

The `ffx product-bundle bisect` tool supports an `--script` argument that takes
an executable file path. During the execution phase of a bisection step, instead
of waiting for the user to manually verify a flashed device, the bisection CLI
tool will invoke this script.

If the script returns an exit code of `0`, the CLI considers the test as passing
(this is a "good" build). If the script returns any non-zero exit code (e.g.,
`1`), the CLI considers the test as failing (this is a "bad" build).

### Example Script Structure

When automating a bisection test, your script will typically need to:

1.  **Prepare the device for flashing:** Reboot the device into a state where
    `ffx` can flash the downloaded product bundle.
2.  **Flash the device:** Use `ffx target flash` to push the downloaded image
    onto the hardware.
3.  **Wait for boot:** Ensure the device successfully boots and reconnects to
    `ffx`.
4.  **Execute the test:** Run the specific test you are trying to bisect (e.g.,
    an `fx test` command, a custom Python script, or pinging an endpoint).
5.  **Return the outcome:** Exit with `0` on success, or non-zero on failure.

Here is a simplified template demonstrating this workflow:

```bash
#!/bin/bash
# A template test script for automated bisection

set -ex

PB_PATH=""

# Parse arguments to find the paths passed by the bisect tool
while [[ "$#" -gt 0 ]]; do
    case $1 in
        --pb) PB_PATH="$2"; shift 2 ;;
        *) shift ;;
    esac
done

# 1. Device-specific setup to enter fastboot/flash mode
# (See Device-Specific Considerations below)
ffx target reboot --bootloader || true
sleep 10

# 2. Flash the device using the current bisection product bundle
ffx target flash -b "$PB_PATH"
sleep 20

# 3. Wait for the device to become reachable again
ffx target wait

# 4. Run the actual test
echo "Running the target test..."
if fx test my_failing_test_component; then
    echo "Test passed!"
    exit 0
else
    echo "Test failed!"
    exit 1
fi
```

## Device-Specific Considerations

Different target hardware (e.g. Vim3, NUC) require different preparation steps
to get the device into a flashable state before `ffx target flash` can be
executed. For internal products, see the README in the v/g repository for more
information.
