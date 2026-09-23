# Use developer overrides for assembly of a Fuchsia product

The Fuchsia build system uses a combination of platform, product, and board
configurations to create a final product bundle. While these configurations
define the canonical version of a product, you may need to make temporary
modifications for testing or debugging. For more conceptual information about
software assembly, see [Software Assembly][software-assembly-concepts].

Developer overrides provide a mechanism to make local-only modifications to a
product's configuration without changing the source files in the Fuchsia tree.

This document explains how you can use developer overrides to modify a product's
configuration.

## Using assembly overrides {:#use-assembly-overrides}

To use developer overrides for assembly, you need to:

1.  **Define the overrides** in a GN target, typically in `//local/BUILD.gn`.
2.  **Apply the overrides** to your build configuration.

For example, to enable a kernel argument, first define the override in
`//local/BUILD.gn`:

```gn
import("//build/assembly/developer_overrides.gni")

assembly_developer_overrides("enable_kernel_debug") {
  kernel = {
    command_line_args = [ "foo" ]
  }
}
```

Then, apply this override to your build configuration using one of the following
methods:

*   [Using `fx set`](#fx-set)
*   [Using `fx args` or `args.gn``](#args-gn)

### Using `fx set` {:#fx-set}

The `fx set` command includes the `--assembly-override` option to specify an
override for the main product assembly (not the test assembly, recovery, etc...).
For example:

Note: The value used for `--assembly-override` is the identifier that you used
in your `//local/BUILD.gn` file.

```gn
fx set core.x64 --assembly-override //local:enable_kernel_debug
```

The `--assembly-override` option supports two formats:

Note: You can provide this option multiple times, but each instance must apply
to a different assembly target. An error is generated if more than one override
target matches the same assembly target.

*   `--assembly-override <overrides_target>`: Applies the overrides to the main
    product assembly.

    For example, the following command applies the `//local:enable_kernel_debug`
    override to the `core.x64` product:

    ```posix-terminal
    fx set core.x64 --assembly-override //local:enable_kernel_debug
    ```

*   `--assembly-override <assembly_target_pattern>=<overrides_target>`: Applies
    the overrides to a specific assembly target pattern.

    For example, to apply overrides defined in `//local:zedboot_overrides` only
    to the `zedboot` assembly, you would run the following:

    ```posix-terminal
    fx set core.x64 --assembly-override '//products/zedboot/*=//local:zedboot_overrides'
    ```

For more information, see the following sections:

*   To learn how to specify the assembly target, see
    [Product Label Patterns](#product-label-patterns).
*   To learn how to define the override target, see
    [Defining an `assembly_developer_overrides` target](#defining-an-assembly_developer_overrides-target).

### Using `fx args` or `args.gn` {:#args-gn}

You can apply overrides by editing your `args.gn` file or using
[`fx args`][fx-args-ref]. The following GN arguments are available:

*   [`product_assembly_overrides_label`](#product-assembly-overrides-labe):
    Specifies a single override target for the main product assembly.
*   [`product_assembly_overrides_contents`](#product-assembly-overrides-contents):
    Defines the overrides inline within `args.gn` for the main product assembly.
*   [`product_assembly_overrides`](#product-assembly-overrides-contents): A list
    that explicitly maps override targets to the assembly targets they apply to.

#### `product_assembly_overrides_label` {:#product-assembly-overrides-label}

The `product_assembly_overrides_label` argument applies the specified overrides
to the main assembly of the selected product. It does not affect other
product assemblies like recovery, zedboot, or tests.

Example `args.gn`:

```gn
import("//products/...")
import("//boards/...")

product_assembly_overrides_label = "//local:my_overrides"
```

#### `product_assembly_overrides_contents` {:#product-assembly-overrides-contents}

The `product_assembly_overrides_contents` argument allows you to define
overrides directly in your `args.gn`. These overrides only apply to the main
product assembly.

Example `args.gn`:

```gn
import("//products/...")
import("//boards/...")

product_assembly_overrides_contents = {
  kernel = {
    command_line_args = [ "kernel.enable-debugging-syscalls=true" ]
  }
}
```

The value uses the same syntax as the `assembly_developer_overrides()` GN
template, see
[Defining an `assembly_developer_overrides` target](#defining-an-assembly_developer_overrides-target).

#### `product_assembly_overrides` {:#product-assembly-overrides}

The `product_assembly_overrides` argument is used to specify overrides for
multiple or non-main assemblies. It uses [GN label patterns][gn-label-patterns]
to match override targets to their corresponding assembly targets.

Example `args.gn`:

Note: You can also place this configuration in `//local/args.gn`, and `fx set`
automatically appends it to the `args.gn` in your build directory.

```gn
import("//products/....")
import("//boards/....")

product_assembly_overrides = [
  {
    # zedboot
    assembly = "//products/zedboot/*"
    overrides = "//local:zedboot_overrides"
  },
  {
    # For assemblies in Bazel, use their product label
    assembly = "//products/minimal/*"
    overrides = "//local:minimal_overrides"
  },
  {
    assembly = "//products/core/*"
    overrides = "//local:enable_kernel_debug"
  },
]
```

## Defining an `assembly_developer_overrides` target {:#defining-an-assembly_developer_overrides-target}

To define a set of developer overrides, use the `assembly_developer_overrides()`
template in a `BUILD.gn` file. It is recommended to place these definitions in
the `//local` directory of your Fuchsia checkout, as this directory is ignored
by git.

Example `//local/BUILD.gn`:

Note: To see available override options, see
[Available override options](#available-override-options).

```gn
import("//build/assembly/developer_overrides.gni")

assembly_developer_overrides("enable_kernel_debug") {
  kernel = {
    command_line_args = [
      "foo",
    ]
  }
}

# Multiple override sets can be defined in the same file.
assembly_developer_overrides("bar_debug") {
  kernel = {
    command_line_args = [ "bar" ]
  }
}
```

## Available override options {:#available-override-options}

The following sections describe the available options that can be configured
within an `assembly_developer_overrides` target:

* [Developer-only assembly options](#developer-only-assembly-options)
* [Platform configuration](#platform-configuration)
* [Kernel command-line arguments](#kernel-command-line-arguments)
* [Additional packages](#additional-packages)
* [Shell commands](#shell-commands)
* [Board configuration](#board-configuration)
* [Compiled packages and components](#compiled-packages-and-components)
* [Appending to lists](#appending-to-lists-with-__append_to__key_)

### Developer-only assembly options {:#developer-only-assembly-options}

These assembly options can only be enabled through developer overrides, and are
not normally allowed in product or board configs.

Example `//local/BUILD.gn`:

```gn
import("//build/assembly/developer_overrides.gni")

assembly_developer_overrides("netboot_with_all_packages") {
  developer_only_options = {
    all_packages_in_base = true
    netboot_mode = true
  }
}
```

*   `all_packages_in_base`: Redirects all `cache` and `on_demand` packages into
    the `base` package set. This is useful for making debugging tools available
    on a device when networking is non-functional.
*   `netboot_mode`: Creates a Zircon Boot Image (ZBI) that includes the FVM/Fxfs
    image inside a ramdisk, allowing the product to be netbooted.

### Platform configuration {:#platform-configuration}

You can override the `platform` configuration of a product assembly
configuration. For more information, see the
[`PlatformSettings` reference][PlatformSettings-config].

The values that you specify are merged with the product's platform configuration
on a field-by-field basis.

Example `//local/BUILD.gn`:

```gn
import("//build/assembly/developer_overrides.gni")

assembly_developer_overrides("enable_sl4f") {
  platform = {
    development_support = {
      include_sl4f = true
      include_netsvc = true
    }
  }
}
```

If you want to append to a list instead of replacing it, see
[Appending to lists with `__append_to_<key>`](#appending-to-lists-with-__append_to__key_).

### Kernel command line arguments {:#kernel-command-line-arguments}

You can add kernel command line arguments. The order of the kernel command
line arguments are not preserved. For a list of available options, see the
[Zircon Kernel Commandline Options][zircon-boot-options] documentation.

Example `//local/BUILD.gn`:

```gn
import("//build/assembly/developer_overrides.gni")

assembly_developer_overrides("bar_debug") {
  kernel = {
    command_line_args = [ "bar" ]
  }
}
```

### Additional packages {:#additional-packages}

You can add developer-specified packages to the `base`, `cache`, `bootfs`, and
`flexible` package sets. The `flexible` package set is placed in `cache` for
`eng` build types and in `base` for `user` and `userdebug` build types.

The template requires specific GN labels for packages and does not use GN
metadata to traverse package groups.

Example `//local/BUILD.gn`:

```gn
import("//build/assembly/developer_overrides.gni")

assembly_developer_overrides("my_custom_base_packages") {
  base_packages = [
    "//some/gn/target/for/a:package",
    "//some/other/target/for/a:package",
    "//third_party/sbase",
  ]
}
```

### Shell commands {:#shell-commands}

To add command-line tools to the Fuchsia shell, you must both add the package
containing the tool and instruct assembly to create the launcher stub for the
component.

Example `//local/BUILD.gn`:

```gn
import("//build/assembly/developer_overrides.gni")

assembly_developer_overrides("my_custom_shell_commands") {
  shell_commands = [
    {
      package = "cp"
      components = [ "cp" ]
    }
  ]
}
```

If making the package available through package discovery isn't sufficient, you
can also add the package to a package set.

Example `//local/BUILD.gn`:

```gn
import("//build/assembly/developer_overrides.gni")
assembly_developer_overrides("my_custom_shell_commands") {
  shell_commands = [
    {
      package = "cp"
      components = [ "cp" ]
    }
  ]

  # This GN target should define a package named "cp".
  base_packages = [
    "//some/gn/target/for/my/package:cp"
  ]
}
```

### Board configuration {:#board-configuration}

You can override board-specific configuration to test changes or new features.

Example `//local/BUILD.gn`:

```gn
import("//build/assembly/developer_overrides.gni")

assembly_developer_overrides("add_new_feature") {
  board = {
    provided_features = [ "fuchsia::new_feature" ]
    filesystems = {
      gpt_all = true
    }
  }
}
```

To append to a list instead of replacing it, see
[Appending to lists with `__append_to_<key>`](#appending-to-lists-with-__append_to__key_).

### Compiled packages and components

You can add developer-specified contents and CML shards to packages and
components that are compiled by assembly.

Example `//local/BUILD.gn`:

```gn
import("//build/assembly/developer_overrides.gni")

assembly_developer_overrides("add_core_shard") {
  compiled_packages = [
    {
      name = "core"
      components = [
        {
          component_name = "core"
          // Paths to _files_, not targets.
          shards = [ "//local/testing.core_shard.cml" ]
        },
      ]
    }
  ]
}
```

### Appending to lists with `__append_to_<key>` {:#appending-to-lists-with-__append_to__key_}

By default, developer overrides replace a list with the new contents that you
specify. To append to a list instead, use the `__append_to_<key>` syntax, where
`<key>` is the name of the list.

For example, given the following board configuration:

```gn
board_configuration("bar") {
  provided_features = [ "fuchsia::feature1", "fuchsia::feature2"]
}
```

You can append a new feature using this override:

```gn
import("//build/assembly/developer_overrides.gni")

assembly_developer_overrides("foo") {
  board = {
    __append_to_provided_features = [ "fuchsia::new_feature" ]
  }
}
```

The resulting configuration will be:

```gn
board: {
  provided_features: [
    "fuchsia::feature1",
    "fuchsia::feature2",
    "fuchsia::new_feature",
  ]
}
```

## Important details

### Assembly warning text

Assembly operations with overrides always produce a warning that details the
overrides being applied. For example:

```gn
WARNING!:  Adding the following via developer overrides from: //local:enable_kernel_debug

  Additional kernel command line arguments:
    foo
```

### Product label patterns {#product-label-patterns}

The mapping between an override target and a product bundle is done through
label pattern matching.

Important: Do not use `//*` as a pattern. It will match every assembly in the
build, including all tests, zedboot, and recovery images.

*   `//some/label/with/wildcard/*`: Matches all assemblies in and under that
    path.
*   `//some/label/with/wildcard:*`: Matches all assemblies in that folder only.

Different assemblies are located at different paths in the build graph. Common
locations include the following:

*   **GN-assembled products**:
    *   `guest`: `//build/images/guest/*`
*   **Bazel-assembled products** (`bringup`, `core`, `minimal`, `terminal`, `workbench`, etc.):
    *   Boards in `fuchsia.git`: `//products/<name>/*`
    *   Vendor boards: `//vendor/<vendor>/products/<name>/*`

[gn-label-patterns]: https://gn.googlesource.com/gn/+/master/docs/reference.md#label_pattern
[PlatformSettings-config]: /reference/assembly/PlatformSettings/index.md
[software-assembly-concepts]: /docs/concepts/software_assembly/overview.md
[zircon-boot-options]: /docs/gen/boot-options.md
[fx-args-ref]: /reference/tools/fx/cmd/args.md
