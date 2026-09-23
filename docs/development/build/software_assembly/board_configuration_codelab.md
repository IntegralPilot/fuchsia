# Codelab: Defining a new board configuration

This codelab walks through the process of defining a new board configuration in
Fuchsia using the Bazel build system. Board configurations are crucial for
specifying the hardware-specific components, drivers, and settings required to
run Fuchsia on a particular device.

## Prerequisites {:#prerequisites}

*   Familiarity with Fuchsia's [software assembly concepts][assembly-concepts].
*   Basic understanding of [Bazel build rules][bazel-concepts].

## What is a board configuration? {:#what-is-a-board-configuration}

A board configuration in Fuchsia encapsulates all the necessary information to
build a system image for a specific piece of hardware. This includes:

*   **Hardware identification:** Name and version.
*   **Code:** Board-specific drivers and platform features.
*   **Storage layout:** How partitions are arranged.
*   **Boot process:** Kernel arguments and device tree information.
*   **Filesystem details:** Options like compression.

Board configurations are defined using the `fuchsia_board_configuration` rule in
`BUILD.bazel` files, typically located within the `//boards` directory.

## Writing a board configuration {:#writing-a-board-configuration}

Let's explore the key attributes of the `fuchsia_board_configuration` rule with
examples.

### Basic setup {:#basic-setup}

Every board configuration needs a name, board name, and partitions config. For
example:

```bazel
# //boards/my_awesome_board/BUILD.bazel
load(
    "@rules_fuchsia//fuchsia:assembly.bzl",
    "fuchsia_board_configuration",
)

fuchsia_board_configuration(
    name = "my_awesome_board",  # Target name for the build system
    board_name = "my_awesome_board",  # Identifier used by tools like ffx
    version = "1.2.3.4",  # Version of the board configuration
    partitions_configuration = "//boards/partitions/my_awesome_board",
    # ... more attributes to come
)
```

### Partitions configuration {:#partitions-configuration}

The `partitions_configuration` field points to a
`fuchsia_partitions_configuration` target, typically defined in a `BUILD.bazel`
file within a subdirectory of `//boards/partitions`. This target defines the
layout and types of partitions for the board, such as where the ZBI,
[VBMeta][vbmeta-readme], and [FXFS][fxfs-rfc] are located.

These images and partitions are often organized into different
[ABR slots][abr-concepts].

For example in `//boards/partitions/my_awesome_board/BUILD.bazel`:

```bazel
load(
    "@rules_fuchsia//fuchsia:assembly.bzl",
    "PARTITION_TYPE",
    "SLOT",
    "fuchsia_bootloader_partition",
    "fuchsia_partition",
    "fuchsia_partitions_configuration",
)

# Standard Fuchsia partitions
fuchsia_partition(
    name = "zircon_a",
    partition_name = "zircon_a",
    slot = SLOT.A,
    type = PARTITION_TYPE.ZBI,
)

fuchsia_partition(
    name = "vbmeta_a",
    partition_name = "vbmeta_a",
    slot = SLOT.A,
    type = PARTITION_TYPE.VBMETA,
)

fuchsia_partition(
    name = "fxfs",
    partition_name = "fvm", # Or "fxfs" depending on board
    type = PARTITION_TYPE.FXFS,
)

# ... (typically define all slots A, B, R for ZBI/VBMeta)

# Board-specific bootloader partition
fuchsia_bootloader_partition(
    name = "my_bootloader",
    image = "//boards/my_awesome_board/firmware:my_awesome_bootloader.img",
    partition_name = "bootloader",

    # The type is used by the board's paver driver to map to a particular
    # partition during Over-the-Air (OTA) updates.
    type = "",
)

# The main configuration, referencing the partition targets
fuchsia_partitions_configuration(
    name = "my_awesome_board",

    # `ffx` compares `hardware_revision` to the fastboot variable `hw-revision`,
    # and refused to flash the device is they do not match. This ensures that
    # the wrong image is not flashed to the wrong board.
    hardware_revision = "my_awesome_board_rev1",
    bootloader_partitions = [
        ":my_bootloader",
    ],
    partitions = [
        ":zircon_a",
        ":vbmeta_a",
        ":fxfs",
        # ... and other partition targets
    ],
)
```

*Explanation:*

*   `fuchsia_partition` defines individual partitions like ZBI, VBMeta, and
    FXFS, specifying their name, type, and slot if applicable.
*   `fuchsia_bootloader_partition` defines bootloader partitions, linking to the
    bootloader image target. Assembly treats bootloader partitions as
    "unslotted".
*   Define a
    [`fuchsia_partitions_configuration`][fuchsia_partitions_configuration_ref]
    target that describes the partitions on the device. This is required for
    all boards.

### Including code: Board-specific vs. platform {:#including-code}

There are two primary ways to include code in your board configuration:

*   [Board Input Bundles (BIBs):](#board-input-bundles) For code tightly coupled
    to the board hardware.
*   [Provided features:](#provided-features) For requesting generic capabilities
    from the Fuchsia platform

#### Board input bundles (BIBs) {:#board-input-bundles}

BIBs are used to package board-specific drivers, configurations, and other files
that are unique to the hardware. These are defined using the
[`fuchsia_board_input_bundle`][fuchsia_board_input_bundle_ref] rule. For example:

```bazel
# //boards/my_awesome_board/BUILD.bazel
load(
    "@rules_fuchsia//fuchsia:assembly.bzl",
    "fuchsia_board_input_bundle",
)

# BIB for a board-specific compass driver
fuchsia_board_input_bundle(
    name = "my-compass",
    bootfs_driver_packages = [
        "//src/devices/board/drivers/my-compass",
    ],
)
```

The BIB can be included in the board configuration using the
`board_input_bundles` attribute. For example:

```bazel
# //boards/my_awesome_board/BUILD.bazel
fuchsia_board_configuration(
    name = "my_awesome_board",
    board_name = "my_awesome_board",
    version = "1.2.3.4",
    partitions_configuration = "//boards/partitions/my_awesome_board",
{{"<strong>"}}
    board_input_bundles = [
        ":my-compass",  # Reference the BIB target
    ],
{{"</strong>"}}
)
```

It is best practice to create a single BIB for each logical group of hardware
that may also be found on other boards. This allow similar boards to share
BIBs. Examples include `compass`, `paver`, `rtc`, etc.

#### Provided features {:#provided-features}

This attribute allows the board to declare that it supports or requires certain
platform-level features. The assembly system can then include the necessary
platform code based on these flags.

This mechanism decouples the board definition from the implementation details
of common platform features. Instead of the board *providing* the code, it
*declares* the need for it. For example:

```bazel
# //boards/my_awesome_board/BUILD.bazel
fuchsia_board_configuration(
    name = "my_awesome_board",
    board_name = "my_awesome_board",
    version = "1.2.3.4",
    partitions_configuration = "//boards/partitions/my_awesome_board",
{{"<strong>"}}
    provided_features = [
        "fuchsia::driver_runtime",
        "fuchsia::vulkan_support",
    ],
{{"</strong>"}}
)
```

In this case, the board is stating it has hardware requiring platform-provided
runtime drivers and Vulkan support.

### Device tree {:#device-tree}

If your board uses a device tree, you can specify the `.dtb` binary target using
the `devicetree` attribute. For example:

```bazel
# In //boards/my_awesome_board/BUILD.bazel
fuchsia_board_configuration(
    name = "my_awesome_board",
    board_name = "my_awesome_board",
    version = "1.2.3.4",
    partitions_configuration = "//boards/partitions/my_awesome_board",
{{"<strong>"}}
    devicetree = "//boards/my_awesome_board/firmware:board.dtb",
{{"</strong>"}}
)
```

### Filesystem options {:#filesystem-options}

The `filesystems` attribute takes a dictionary to configure various aspects of
the images, such as ZBI compression. For a full list of available options, see
[BoardFilesystemConfig][board-filesystem-config]. For example:

```bazel
# In //boards/my_awesome_board/BUILD.bazel
fuchsia_board_configuration(
    name = "my_awesome_board",
    board_name = "my_awesome_board",
    version = "1.2.3.4",
    partitions_configuration = "//boards/partitions/my_awesome_board",
{{"<strong>"}}
    filesystems = {
        "zbi": {
            "compression": "zstd.max",
        },
    },
{{"</strong>"}}
)
```

You can also configure vbmeta signing keys in the `filesystems` attribute. For
example:

```bazel
# In //boards/my_awesome_board/BUILD.bazel
fuchsia_board_configuration(
    name = "my_awesome_board",
    board_name = "my_awesome_board",
    version = "1.2.3.4",
    partitions_configuration = "//boards/partitions/my_awesome_board",
{{"<strong>"}}
    filesystems = {
        "vbmeta": {
            "key": "//path/to/my:vbmeta_private_key",
            "key_metadata": "//path/to/my:vbmeta_key_metadata",
        },
    },
{{"</strong>"}}
)
```

### Post-processing script {:#post-processing-script}

If your board requires additional image processing steps after the standard
assembly (e.g., to create a vendor-specific boot image format), you can use the
`post_processing_script` attribute.

1.   Define a
[`fuchsia_post_processing_script`][fuchsia_post_processing_script_ref] target.
For example:

    ```bazel
    # In //boards/my_awesome_board/BUILD.bazel
    load(
        "@rules_fuchsia//fuchsia:assembly.bzl",
        "fuchsia_post_processing_script",
    )

    fuchsia_post_processing_script(
        name = "my_post_processing_script",
        post_processing_script_path = "tools/sign_image.sh",
        post_processing_script_args = [
            "-i", "zbi",
            "-o", "zbi.signed",
        ],
        post_processing_script_inputs = {
            "//path/to/keys:private_key": "keys/private_key",
        },
    )
    ```

2.   Reference it in your board configuration. For example:

    ```bazel
    # In //boards/my_awesome_board/BUILD.bazel
    fuchsia_board_configuration(
        name = "my_awesome_board",
        board_name = "my_awesome_board",
        version = "1.2.3.4",
        partitions_configuration = "//boards/partitions/my_awesome_board",
    {{"<strong>"}}
        post_processing_script = ":my_post_processing_script",
    {{"</strong>"}}
    )
    ```

## Next steps {:#next-steps}

This codelab covered the basics of defining a new board configuration using
Bazel in Fuchsia. You've seen how to use
[`fuchsia_board_configuration`][fuchsia_board_configuration_ref] and related
rules to specify everything from drivers to filesystem options.

To continue learning, you can:

*   Explore the existing board configurations in the [`//boards`][boards-dir] directory,
    paying attention to the `BUILD.bazel` files in subdirectories like `x64`,
    `arm64`, and `vim3`.
*   Dive deeper into [Fuchsia Software Assembly Concepts][assembly-concepts].

<!-- Reference links -->

[assembly-concepts]: /docs/concepts/software_assembly/overview.md
[bazel-concepts]: /docs/development/build/bazel_concepts/project_layout.md
[boards-dir]: https://cs.opensource.google/fuchsia/fuchsia/+/main:boards
[board-filesystem-config]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/lib/assembly/images_config/src/board_filesystem_config.rs
[fuchsia_partitions_configuration_ref]: https://fuchsia.dev/reference/bazel_sdk/fuchsia_partitions_configuration
[fuchsia_board_configuration_ref]: https://fuchsia.dev/reference/bazel_sdk/fuchsia_board_configuration
[fuchsia_board_input_bundle_ref]: https://fuchsia.dev/reference/bazel_sdk/fuchsia_board_input_bundle
[fuchsia_post_processing_script_ref]: https://fuchsia.dev/reference/bazel_sdk/fuchsia_post_processing_script
[vbmeta-readme]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/lib/assembly/vbmeta
[fxfs-rfc]: /docs/contribute/governance/rfcs/0136_fxfs.md
[abr-concepts]: /docs/glossary/README.md#abr
