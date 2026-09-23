# ffx Analytics Events

The `ffx` tool emits various analytics events for diagnostic and quality tracking purposes. This document lists the core events emitted by `ffx` and documents their meaning. Analytics collection respects the user's opt-in status.

## Core Events

* **`invoke`**: Emitted when an `ffx` command is executed. It captures information about the command invocation, including execution time, exit code, any error message, and the underlying subcommand. Depending on the user's opt-in level for enhanced analytics, command arguments may be fully redacted or partially included.

## Target and Connection Events

* **`ffx_target_list_devices`**: Emitted when the `ffx target list` command executes. Records the number of devices discovered (as the `devices` dimension) along with the type of query used (e.g., no filter, or by nodename).
* **`ffx_target_connection`**: Emitted when establishing a connection to a Fuchsia target. The action parameter indicates the type of connection being attempted.
* **`ffx_connection_mode`**: Emitted concurrently with connection establishment to track the specific connectivity mode or strategy used for interacting with the target.
* **`ffx_rcs_proxy`**: Emitted when `ffx` constructs an Overnet connection to a target's Remote Control Service (RCS). The action specifies the proxy connection strategy employed (e.g., mdns vs target).

## Flashing and Hardware Events

* **`ffx_flash`**: Emitted when writing a partition block to a device (typically during `ffx target flash`). Custom dimensions track the `partition_name`, `product_name`, `board_name`, `file_size`, and the `flash_time` taken to write the partition.

## Host Checks

* **`preflight`**: Emitted by platform preflight checks used to verify that the host machine satisfies requirements for certain commands (such as starting an emulator). The action field captures the result (`completed_success`, `completed_warning`, `completed_failure_recoverable`, `completed_failure`), and the event may capture environmental properties such as the host's graphics driver context.

## Diagnostic and Error Telemetry

* **`ffx_diagnostics_failure`**: Sent when the tool detects specific diagnostic, workflow, or architectural errors during operation. The `category_name` dimension captures the precise failure point, such as:
    * `target_hdl_in_bad_state`: The internal target handle structure was queried in an invalid state.
    * `target_no_netwrk`: Action was attempted on a target that doesn't appropriately expose networking.
    * `target_addr_bad_scope`: An IPv6 target address could not be used because its network scope is invalid.
    * `build_fdomain_cmd`: Failed to construct the parameters necessary for a tool fdomain command.
    * `open_hwinfo_comp`: Failed to connect to the hardware info component on the remote device.
    * `hwinfo_getinfo`: FIDL error encountered when trying to retrieve hardware information data safely.
    * `non_fastboot_target_hdl`: Attempted a fastboot operation on a device handle that isn't in the fastboot state.
    * `query_serialno`: Failed to reliably query the device for its hardware serial number.

## Developer Tools

* **`ffx_playground_cmd`**: Emitted when executing commands inside the experimental `ffx playground` shell. Records the underlying command in the `type` dimension and logs whether its execution succeeded or failed.
