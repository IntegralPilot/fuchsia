# Diagnostics Persistence: Saving Inspect across reboot

Diagnostics Persistence automatically captures and stores Inspect data across
device reboots. This can be used to preserve anything that can be written to
Inspect.

## Overview {#overview}

*   **Automatic collection in development**: On `eng` and `userdebug` builds,
    all Inspect data across the system is automatically persisted across
    reboots. No configuration required.

*   **Production (`user`) builds require privacy configuration**: On production
    devices, persisted Inspect data must be explicitly allowlisted in a
    product's `previous_boot` pipeline (for example, in go/previous-boot-pipeline)
    and undergo privacy review.

*   **Available in Feedback & Crash Reports**: Persisted Inspect data is
    automatically ingested by Feedback and included in snapshot archives
    (as `inspect.previous_boot.json`) when generating feedback or crash reports.

## Field collection on user builds {#field-collection-on-user-builds}

On production (`user`) builds, diagnostic data exfiltration is restricted for
user privacy. To persist specific Inspect properties or trees on production
devices, you must add your selectors to the product's `previous_boot` pipeline
and complete a privacy review.

The privacy requirements for Persistence are functionally identical to
go/tq-feedback-privacy.

### 1. Identify Inspect selectors {#identify-inspect-selectors}

Determine the exact Inspect selectors for the properties or nodes you need.
You can inspect your component on a running device or emulator using
`ffx inspect`.

### 2. Add selectors to the previous_boot pipeline {#add-selectors-to-pipeline}

Create or update the selector configuration for your component in the product
repository's pipeline directory:

1.  Create a component directory under the product's `previous_boot` pipeline:

    ```none
    //vendor/*/privacy/pipelines/previous_boot/<component_name>/inspect/
    ```

2.  Add a `.cfg` file (e.g. `<component_name>.cfg`) containing the exact
    selectors to persist:

    ```none
    core/my_component:root/my_node:error_count
    core/my_component:root/my_node:last_failure_reason
    ```

3.  Update the corresponding `BUILD.bazel` or pipeline list to include your
    new configuration.

### 3. Submit for privacy review {#submit-for-privacy-review}

Refer to go/tq-feedback-privacy.

## Accessing persisted data {#accessing-persisted-data}

### Feedback reports and snapshots {#feedback-reports-and-snapshots}

When a feedback report or crash snapshot is taken after a reboot, Feedback
automatically retrieves the previous boot's Inspect data from Persistence.
The resulting snapshot contains:

*   `inspect.previous_boot.json`: The complete or filtered Inspect JSON tree
    from the prior boot.

## Technical architecture and behavior {#technical-architecture-and-behavior}

Under the hood, Diagnostics Persistence operates as a scheduled and
event-driven pipeline snapshotting service:

```none {:.devsite-disable-click-to-copy}
┌───────────────────────────┐
│ Archivist                 │
│ ArchiveAccessor           │
│ .previous_boot pipeline   │
└─────────────┬─────────────┘
              │ Snapshot query
              ▼
┌───────────────────────────┐   writes active snapshot
│ Diagnostics Persistence   ├───► /cache/active/active.json
│                           │     /cache/active/metadata.json
│ • Periodic (every 300s)   │
│ • Low battery trigger     │   on reboot: rotate
│ • Wait for update check   ├───► /cache/previous_boot/active.json
└─────────────┬─────────────┘     /cache/previous_boot/metadata.json
              │ Serves via FIDL
              ▼
┌───────────────────────────┐
│ PreviousBootDataProvider  │
│ (e.g. Feedback component) │
└───────────────────────────┘
```

### 1. Archivist previous_boot pipeline {#archivist-previous-boot-pipeline}

Persistence connects to `fuchsia.diagnostics.ArchiveAccessor.previous_boot`.
Archivist dynamically applies the allowlist selectors configured for the
pipeline, or bypasses filtering when `DISABLE_FILTERING.txt` is present (such
as in `eng` and `userdebug` builds).

### 2. Snapshot triggers {#snapshot-triggers}

Persistence records active system snapshots using two triggers:

*   **Periodic Interval**: Runs every $N$ seconds (configured by
    `fuchsia.diagnostics.persist.PersistencePeriodSeconds`, default 300 seconds).

*   **Low-Battery Trigger**: Connects to
    `fuchsia.power.battery.BatteryManager` to monitor battery status. If
    battery drops to or below
    `fuchsia.diagnostics.persist.LowBatteryThresholdPercent` (default 10%), an
    immediate snapshot is captured to preserve state prior to impending
    shutdown.

### 3. Active-to-previous boot rotation {#active-to-previous-boot-rotation}

*   Active snapshots and metadata are saved to `/cache/active/active.json` and
    `/cache/active/metadata.json`.

*   On startup, Persistence atomically cleans `/cache/previous_boot` and moves
    `/cache/active` to `/cache/previous_boot`.

*   Persisted data is retained strictly for the single previous boot cycle.

### 4. Software update check gating {#software-update-check-gating}

To protect against persistent crash loops across updates, Persistence
registers with `fuchsia.update.Listener` and withholds serving previous boot
data until the first post-boot software update check completes. On `eng` and
`userdebug` builds, this check can be skipped (`skip_update_check: true`).

## Assembly configuration {#assembly-configuration}

Persistence parameters can be customized in product assembly configuration
under the `diagnostics.persistence` section:

*   `persistence_period_seconds` (integer, default: `300`):
    Duration in seconds between periodic snapshots on
    `ArchiveAccessor.previous_boot`.

*   `low_battery_threshold_percent` (integer, default: `10`):
    Battery percentage threshold that triggers an immediate active snapshot.

*   `skip_update_check` (boolean, default: `false` on user, `true` on
    userdebug/eng):
    If `true`, does not wait for the post-boot update check before publishing
    previous boot data. Always `false` on user builds.

## FAQ {#faq}

### Does Persistence work with Lazy Nodes? {#lazy-nodes}

Yes. Because Persistence requests a snapshot from Archivist via
`ArchiveAccessor`, Archivist actively evaluates all Lazy Nodes that match the
pipeline's active selectors at the time of each snapshot.

### Where did .persist configuration files go? {#legacy-config-files}

Legacy `.persist` files and per-tag fetch scheduling have been replaced by the
Archivist `previous_boot` pipeline. Components no longer publish separate
`.persist` files or re-export persisted data in the live Inspect hierarchy
under `core/diagnostics/persistence:root/persist`. All previous boot Inspect
is unified into a single snapshot file served directly to Feedback.

