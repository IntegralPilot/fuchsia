# input_pipeline > Integration

Reviewed on: 2022-03-22

This document describes how to integrate the input pipeline within a
larger Rust program. For example, the input pipeline is integrated with [Scene Manager](https://cs.opensource.google/fuchsia/fuchsia/+/main:src/ui/bin/scene_manager/) for some builds.

## Requirements

The code that deals with the input pipeline must run on a `LocalExecutor`. For
existing code, this is already true. [SceneManager](/src/ui/bin/scene_manager/README.md)
uses `fuchsia::main`, so it uses a `LocalExecutor`.

## Alternatives

A program might want to isolate itself from the risk that a bug in the input
pipeline library (e.g. infinite loop) will keep the rest of the program from
making progress. Such programs can

1. Spawn a dedicated thread to run a `LocalExecutor` which creates and runs
   the input pipeline, OR
1. Run the input pipeline as a standalone component, and communicate with
   the pipeline over FIDL.

## Component Manifest

The library provides a [client shard](../meta/client.shard.cml) to ensure
proper routing is available. This shard can be included in the integrating
component's manifest file.

## Structured Configuration

The input pipeline uses structured configuration by declaring a `config`
value in the integrating component's manifest, such as in
`//src/ui/bin/scene_manager/meta/scene_manager.cml`:

```
config: {
    supported_input_devices: {
        type: "vector",
        element: {
            type: "string",
            max_size: 12,
        },
        max_count: 6,
    },
},
```

This value can also be set in product assembly via the following
configuration schema:

```
product_assembly_configuration("my_product") {
  platform = {
    input = {
      supported_input_devices = [
        "button",
        "touchscreen",
      ]
    }
  }
}
```

It can also be set to a default value such as for testing:

```
fuchsia_structured_config_values("test_config") {
  cm_label = ":manifest"
  values = {
    supported_input_devices = [
      "button",
      "keyboard",
      "lightsensor",
      "mouse",
      "touchscreen",
    ]
  }
}
```
