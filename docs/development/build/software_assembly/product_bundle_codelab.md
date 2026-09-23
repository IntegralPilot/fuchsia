# Codelab: Defining and building a product bundle

This codelab walks through the process of defining and building a new product
bundle in Fuchsia using the Bazel build system. A product bundle is the
distributable artifact that contains all the images and metadata needed to
flash, update, or emulate a Fuchsia product.

As part of this process, you will also define a product configuration to specify
the software features, settings, and packages that make up the product.

## Prerequisites {:#prerequisites}

*   Familiarity with Fuchsia's [software assembly concepts][assembly-concepts].
*   Basic understanding of [Bazel build rules][bazel-concepts].

## What is a product configuration? {:#what-is-a-product-configuration}

A product configuration in Fuchsia defines the user experience for a specific
product. It is distinct from a [board configuration][board-config], which
specifies hardware-specific details. A product configuration typically includes:

*   **Platform settings:** Settings for the underlying Fuchsia operating
    system (e.g., build type, enabled system features).
*   **Product settings:** Settings specific to the product experience
    (e.g., session shell, specific packages).

Product configurations are defined using the `fuchsia_product_configuration`
rule in `BUILD.bazel` files.

## Writing a product configuration {:#writing-a-product-configuration}

Let's explore how to define a product configuration and use it to build a
system image.

### Basic setup {:#basic-setup}

A product configuration is primarily a JSON object (represented as a dictionary
in Starlark) passed to the `fuchsia_product_configuration` rule.

```bazel
# //products/my_product/BUILD.bazel
load(
    "@rules_fuchsia//fuchsia:assembly.bzl",
    "fuchsia_product_configuration",
)

fuchsia_product_configuration(
    name = "product_config",
    product_config_json = {
        "platform": {
            "build_type": "eng",
            "feature_set_level": "standard",
        },
        "product": {},
    },
)
```

### Platform settings {:#platform-settings}

The `platform` key in the configuration dictionary controls various aspects of
the Fuchsia system. Common fields include:

*   `build_type`: Controls the security and optimization level.
    *   `eng`: Engineering build. This build is best for debugging as it has assertions
         enabled and optimizations disabled.
    *   `userdebug`: This build is optimized like the `user` build, but with
        some debug features and tools enabled.
    *   `user`: Production build. This build is fully optimized and secure.
*   `feature_set_level`: Defines the baseline set of features included in the system.
    *   `embeddable`: Minimal subset of bootstrap. This is optimized for memory constrained
         environments and does not self-update.
    *   `bootstrap`: Bootable, serial-only. Only the `/bootstrap` realm. No
        netstack or storage drivers. Primarily used for board-level bringup.
    *   `utility`: Smallest configuration with the `/core` realm. Best for
        utility-type systems like recovery.
    *   `standard`: A "full Fuchsia" configuration. Includes netstack and
        self-update capabilities. This is the default.
*   `storage`: Configures the filesystem (e.g., Fxfs) and storage layout.

For a full list of platform settings, see the
[PlatformSettings documentation][platform-settings-docs].

For example, a Fuchsia product configuration with platform settings may look
like the following:

```bazel
fuchsia_product_configuration(
    name = "product_config",
    product_config_json = {
{{"<strong>"}}
        "platform": {
            "build_type": "eng",
            "feature_set_level": "standard",
            "storage": {
                "filesystems": {
                    "volume": "fxfs",
                },
            },
        },
        "product": {},
{{"</strong>"}}
    },
)
```

### Product settings {:#product-settings}

The `product` key is used for higher-level product settings.

One of the most important settings is the **[session][session-docs]**. A session
is the top-level component that defines the product's user experience (e.g., the
graphical shell or main application).

To use a session, you must specify its URL and ensure the package containing the
component is included in the **base packages**.

**Base packages** are the set of packages that are included in the system image.
They are available immediately at boot, are immutable (read-only), and are
updated as part of the system OTA.

For example, a Fuchsia product configuration with product settings may look like
the following:

Example:

```bazel
fuchsia_product_configuration(
    name = "product_config",
    product_config_json = {
        "platform": {
            "build_type": "eng",
            "feature_set_level": "standard",
        },
{{"<strong>"}}
        "product": {
            "session": {
                "url": "fuchsia-pkg://fuchsia.com/my_session#meta/my_session.cm",
            },
        },
    },
    base_packages = [
        "//src/my_session:my_session_package",
    ],
{{"</strong>"}}
)
```

For a full list of product settings, see the
[ProductSettings documentation][product-settings-docs].

### Assembling the product image {:#assembling-the-product-image}

Once you have a `fuchsia_product_configuration`, you can use the
`fuchsia_product` rule to assemble the final system image. This rule combines
your product configuration with a specific board configuration.

```bazel
load(
    "@rules_fuchsia//fuchsia:assembly.bzl",
    "fuchsia_product",
)

fuchsia_product(
    name = "image.x64",
    board_config = "//boards:x64",
    platform_artifacts = "//build/bazel/assembly/assembly_input_bundles:platform_eng",
{{"<strong>"}}
    product_config = ":product_config",
{{"</strong>"}}
)
```

*   `board_config`: Points to the [board configuration][board-config] target.
*   `platform_artifacts`: Points to the bundle of platform artifacts (AIBs) to
    use.

### Creating a product bundle {:#creating-a-product-bundle}

To run your product on a device or emulator, you need to package it into a
Product Bundle using the `fuchsia_product_bundle` rule.

```bazel
load(
    "@rules_fuchsia//fuchsia:assembly.bzl",
    "fuchsia_product_bundle",
)

fuchsia_product_bundle(
    name = "my_product.x64",
    product_bundle_name = "my_product.x64",
{{"<strong>"}}
    main = ":image.x64",
{{"</strong>"}}
)
```

This target produces the artifacts needed for `ffx product-bundle` commands.

For example:

*   **Run in emulator:**

    ```posix-terminal
    ffx emu start my_product.x64
    ```

*   **Flash to device:**

    ```posix-terminal
    ffx target flash -b my_product.x64
    ```

## Registering with the build system {:#registering-with-the-build-system}

To build your product bundle, it must be registered with the GN build system.
This involves the following steps:

* [Registering with the build system](#bridging-to-gn)
* [Building the product](#building-the-product)

### Bridging to GN {:#bridging-to-gn}

Fuchsia's build system uses GN, but your product bundle is defined in Bazel. You
need to use the `bazel_product_bundle` GN template to create a bridge.

Note: The `bazel_inputs_from_gn` list ensures that necessary board artifacts
(like the drivers and bootloader) are available to Bazel. The
`allow_eng_platform_bundle_use` flag is required because this example uses an `eng`
build type in the product configuration.

Create a `BUILD.gn` file in your product directory (e.g.,
`//products/my_product/BUILD.gn`):

```gn
# //products/my_product/BUILD.gn
import("//build/bazel/assembly/bazel_product_bundle.gni")

bazel_product_bundle("my_product.x64") {
  testonly = true
  product_bundle_name = "my_product.x64"
  bazel_product_bundle_target = ":my_product.x64"
  bazel_product_image_target = ":image.x64"
  bazel_inputs_from_gn = [
    "//boards/x64:x64.bazel_input",
  ]
  allow_eng_platform_bundle_use = true
}
```

### Adding to allowlists {:#adding-to-allowlists}

You may need to add your new product to the `bazel_action_allowlist` in
`//build/bazel/BUILD.gn` to avoid visibility errors:

```gn
# //build/bazel/BUILD.gn
group("bazel_action_allowlist") {
  visibility = [
    # ...
    "//products/my_product:*",
  ]
  # ...
}
```

You may also need to add it to the `non_hermetic_deps` visibility list in `//build/BUILD.gn`:

```gn
# //build/BUILD.gn
group("non_hermetic_deps") {
  visibility = [
    # ...
    "//products/my_product:*",
  ]
  # ...
}
```

This GN target (`:my_product.x64`) now represents your Bazel product bundle
in the GN build graph.

### Adding to available products {:#adding-to-available-products}

Finally, add this new GN target to the global list of product bundles in
`//products/BUILD.gn` to make it discoverable by `fx`:

```gn
# //products/BUILD.gn
group("product_bundles") {
  testonly = true
  deps = [
    # ... other products
    "//products/my_product:my_product.x64",
  ]
}
```

## Building the product {:#building-the-product}

Fuchsia supports multi-product builds, meaning you can have multiple products
available in the same build directory.

### Configuring the build

Configure your build environment using `fx set`. You should use a product
configuration that enables the Bazel build system, such as `fuchsia.x64`:

```posix-terminal
fx set fuchsia.x64
```

### Setting the main product

To work with your specific product, set it as the "main" product bundle. This
configures `fx` tools to target your product by default:

```posix-terminal
fx set-main-pb my_product.x64
```

### Building the product

To build your currently selected main product (and its dependencies):

```posix-terminal
fx build
```

If you want to explicitly build a specific product bundle regardless of the main
setting:

```posix-terminal
fx build my_product.x64
```

### Switching products

You can switch the active "main" product bundle at any time without re-running
`fx set` by running `fx set-main-pb` again:

```posix-terminal
fx set-main-pb minimal.x64
```

### GN Arguments vs Assembly

In a multi-product build environment, all products share the same GN arguments
(defined by `fx set`). Therefore, you cannot use `fx set ... --args` to
configure product-specific features. Instead, all product configuration must
be done via the `fuchsia_product_configuration` Bazel rule as demonstrated in
this codelab.

## Next steps {:#next-steps}

This codelab covered the basics of defining and building a new product bundle.

To continue learning, you can:

*   Explore existing product configurations in [`//products`][products-dir].
*   Learn more about [Board configurations][board-config].

<!-- Reference links -->

[assembly-concepts]: /docs/concepts/software_assembly/overview.md
[bazel-concepts]: /docs/development/build/bazel_concepts/project_layout.md
[board-config]: /docs/development/build/software_assembly/board_configuration_codelab.md
[products-dir]: https://cs.opensource.google/fuchsia/fuchsia/+/main:products
[session-docs]: /docs/development/sessions/roles-and-responsibilities.md
[platform-settings-docs]: https://fuchsia.dev/reference/assembly/PlatformSettings
[product-settings-docs]: https://fuchsia.dev/reference/assembly/ProductSettings
