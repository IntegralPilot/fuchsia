# Kernel in the build

## Commandline options {#options}

Kernel commandline options can be added to a product assembly either indirectly
(as a platform implementation detail), or directly by a developer for local
debug and testing.

### Specifying options in board or product files

Products and boards can only add kernel commandline options indirectly, via
product assembly:

- As part of an [Assembly Input Bundle](/bundles/assembly/BUILD.gn).
- As part of the [platform configuration rules](/src/lib/assembly/platform_configuration/src/subsystems/)
  in assembly.

### Specifying options locally

For local development, a list of strings that should be appended to the kernel
command line can be specified as part of
[assembly developer overrides](http://go/fuchsia-assembly-overrides).

This takes two steps:

#### 1. Define an assembly overrides target

Define an assembly overrides target in `//local/BUILD.gn` or other `BUILD.gn`
file under `//local/*`.

```gn
import("//build/assembly/developer_overrides.gni")

assembly_developer_overrides("custom_kernel_args") {
  kernel = {
    command_line_args = [ "foo" ]
  }
}
```

This only needs to be done once per set of overrides, and can then be re-used
with any product assembly or any outdir in your build.

The next step is to tell GN which developer overrides target to use for your
product.

#### 2a. Use that target with the "main" product assembly

For the "main" product assembly (the one named in your `fx set` command), you
tell GN to use the above-defined product assembly target with the following:

```posix-terminal
fx set ... --assembly-override //local:custom_kernel_args
```

Alternatively, an existing `args.gn` file can be modified by running `fx args`
and adding or modifying a line as follows:

```gn
product_assembly_overrides_label = "//local:custom_kernel_args"
```

#### 2b. Product assemblies other than the "main" assembly

A slightly more verbose mechanism is required for assemblies other than the
"main" product assembly, such as the zedboot or recovery assemblies.  This is
done using a wildcard-based pattern-match:

```posix-terminal
fx set ... --assembly-override '//products/zedboot/*=//local:custom_kernel_args'
```

or

```posix-terminal
fx set ... --assembly-override '//products/microfuchsia/*=//local:custom_kernel_args'
```

When dealing with multiple product assemblies, it's easier to specify this
directly in `args.gn`:

```gn
product_assembly_overrides = [
  {
    # For assemblies in Bazel, use their product label
    assembly = "//products/core/*"
    overrides = "//local:custom_kernel_args"
  },
  {
    assembly = "//products/zedboot/*"
    overrides = "//local:zedboot_overrides"
  },
  {
    assembly = "//products/microfuchsia/*"
    overrides = "//local:zedboot_overrides"
  }
]
```

Note that when using a `//vendor/*` board, the product assembly target will also
be in the `//vendor` repo (e.g `//vendor/<foo>/products/minimal`).
