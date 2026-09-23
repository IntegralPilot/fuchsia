---
name: driver-hot-reload
description: >
  Iterates on Fuchsia drivers quickly by loading changes without a full flash
  workflow. Use when developing and testing driver changes locally, switching
  between base and universe drivers, or restarting driver hosts. Don't use for
  production deployment or when full system reflash is required.
---

# Driver Hot Reload

## Create a Secondary Package

Check if a secondary package for hot reload already exists.

#### **If** a secondary package already exists:
Reuse the existing target name for the subsequent steps.

#### **Otherwise** (If no secondary package exists):
Follow the steps below depending on whether the driver is defined in GN or
Bazel.

##### **If** the driver is defined in GN:
Create a secondary `fuchsia_package` target in the `BUILD.gn` file. This target
must be identical to the original `fuchsia_package`, except it needs a different
name.

```gn
# Original package
fuchsia_package("my-driver") {
  deps = [ ":my-driver-component" ]
}

# Secondary package for hot reload
fuchsia_package("my-driver-universe") {
  package_name = "my-driver-universe"
  deps = [ ":my-driver-component" ]
}
```

##### **If** the driver is defined in Bazel:
Create a secondary `fuchsia_package` target in the `BUILD.bazel` file.

```python
fuchsia_package(
    name = "my-driver-universe",
    package_name = "my-driver-universe",
    components = [":my-driver-component"],
    fuchsia_api_level = "HEAD",
    package_repository_name = "fuchsia.com",
    visibility = ["//visibility:public"],
)
```

## Add the Package to the Build

#### **If** the driver is defined in GN:
Include the secondary package in the build as a universe package. Use the `fx
add-test` command to add the target to the universe package labels:

```bash
fx add-test --target-list universe_package_labels //path/to/driver:my-driver-universe
```

#### **Otherwise** (If the driver is defined in Bazel):
This step is **not needed**.

## Build the Driver

#### **If** the driver is defined in GN:
Run a build to generate the new package:

```bash
fx build
```

#### **Otherwise** (If the driver is defined in Bazel):
Build the target directly using `fx bazel`:

```bash
fx bazel build --config=fuchsia //src/path/to/driver:my-driver-universe
```

## Start the Package Server

Hot reload requires the package server to be running to serve the universe
package to the device.

Verify that the package server is running. If not, start it:

```bash
fx serve --background
```

## Publish the Package

#### **If** the driver is defined in GN:
This step is **not needed**. The build system automatically publishes packages
in `universe_package_labels`.

#### **Otherwise** (If the driver is defined in Bazel):
The build generates a `package_manifest.json` file in the Bazel output
directory. Navigate to the Bazel execution root and publish the package:

```bash
# CWD: out/fuchsia_internal.arm64-balanced/gen/build/bazel/output_base/execroot/_main
ffx repository publish --package bazel-out/fuchsia_arm64-opt-ST-.../bin/src/path/to/driver/my-driver-universe_fuchsia_package_pkg/package_manifest.json <fuchsia-root>/out/fuchsia_internal.arm64-balanced/amber-files
```

> [!NOTE]
> The path to the manifest in `bazel-out` may contain a hash specific to the build configuration. Check the output of the `fx bazel build` command to find the exact path.

## Swap the Base Driver / Switching Drivers

To dynamically switch between a base driver and a universe driver during
development:

### Switch to Universe Driver

1.  **First time only**: Ensure the universe driver is included in the build as
    a universe package (only needed once per session):
    ```bash
    fx add-test --target-list universe_package_labels //path/to/driver:<universe-driver-name>
    fx build
    ```
2.  Disable the active base driver:
    ```bash
    ffx driver disable fuchsia-pkg://fuchsia.com/<base-driver-name>#meta/<base-driver-name>.cm
    ```
3.  **First time only**: Register the universe driver:
    ```bash
    ffx driver register fuchsia-pkg://fuchsia.com/<universe-driver-name>#meta/<universe-driver-name>.cm
    ```
4.  **Subsequent times**: Enable the universe driver (if previously disabled):
    ```bash
    ffx driver enable fuchsia-pkg://fuchsia.com/<universe-driver-name>#meta/<universe-driver-name>.cm
    ```

### Switch Back to Base Driver

1.  Disable the universe driver:
    ```bash
    ffx driver disable fuchsia-pkg://fuchsia.com/<universe-driver-name>#meta/<universe-driver-name>.cm
    ```
2.  Enable the base driver:
    ```bash
    ffx driver enable fuchsia-pkg://fuchsia.com/<base-driver-name>#meta/<base-driver-name>.cm
    ```

## Verify the Driver is Running

Verify that the universe driver is loaded and bound to the node.

List the loaded drivers to confirm the universe package URL is shown:

```bash
ffx driver list --loaded
```

Check the node list to ensure the node is owned by the universe driver:

```bash
ffx driver node list -v
```

## Restart the Driver

After making changes to the driver source code, perform a hot reload by
restarting the driver:

```bash
ffx driver restart fuchsia-pkg://fuchsia.com/the-universe-driver#meta/the-driver.cm
```

## Common Pitfalls

* **Package Server**: If the package server is not running, the device will fail
  to resolve the universe package with `UnavailableRepoMetadata` or similar
  errors.
* **Colocated Drivers (Devicetree)**: If the driver uses the
  `fuchsia,driver-host` property in devicetree to colocate with drivers across
  the topology, hot reload may not work correctly. The `driver_manager` restart
  logic assumes a tree structure for colocation and may fail to find a single
  "root" node for the host.
  * **Workaround**: Temporarily disable the `fuchsia,driver-host` property in
    the devicetree for the target driver and reflash the device. This allows hot
    reload to work but disables driver transport across those nodes.
* **Blast Radius of Restart**: Restarting a driver takes down the entire driver
  host it lives in, as well as all **children drivers** (descendants) in the
  node topology. All affected drivers must have robust start and stop logic to
  handle the restart gracefully.
  * Use `ffx driver host list` to check which drivers are colocated in the same
    host.
  * To find children drivers:
    1.  Find the moniker bound to the driver: `ffx driver node list -v | grep
        <driver-url>`
    2.  List descendants of that moniker: `ffx driver node list -o
        descendants:<moniker> -v`
* **Active Driver URLs (BootFS vs. Packages)**: Active drivers can be either
  `fuchsia-boot:///` (read-only bootfs) or `fuchsia-pkg://`. Always reference
  the active driver by its exact loaded URL when inspecting or disabling it,
  and deploy your updated code using a secondary universe package.
