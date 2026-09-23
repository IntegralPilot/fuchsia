# escher-examples

These examples show off how to build an application using Escher, the Vulkan rendering engine
powering Scenic.  These run both on Linux and Fuchsia.

## How to run the examples

It can run either directly on the frame buffer or with Scenic.

## fb mode

Add the following args to your `fx set`:

```
fx set core.x64 --with //src/ui/examples/escher --args='core_realm_shards += [ "//src/ui/examples/escher:escher_examples_shard" ]'
```

Build the product, start the emulator/device. Ensure that nothing have already claimed the display,
i.e. Scenic or another fb app like vk_cube. Then run the following commands:

```
ffx component create /core/escher-examples:waterfall fuchsia-pkg://fuchsia.com/escher_waterfall#meta/escher_waterfall_on_fb.cm
ffx component create /core/escher-examples:rainfall fuchsia-pkg://fuchsia.com/escher_rainfall#meta/escher_rainfall_on_fb.cm
```

Now start one of the examples:

```
ffx component start /core/escher-examples:waterfall
```

```
ffx component start /core/escher-examples:rainfall
```

## Scenic mode

Add the following args to your `fx set`:

```
fx set workbench_eng.x64 --with //src/ui/examples/escher
```

Build the product, start the emulator/device and then run the following commands to start the examples:

```
ffx session add fuchsia-pkg://fuchsia.com/escher_waterfall_on_scenic#meta/escher_waterfall_on_scenic.cm
```

```
ffx session add fuchsia-pkg://fuchsia.com/escher_waterfall_on_scenic#meta/escher_rainfall_on_scenic.cm
```
