# Scene Manager

The Scene Manager component creates and configures a Scenic scene graph, and an input pipeline
on launch.

Clients can connect to `fuchsia.session.scene.Manager` to interact with the scene graph. Currently,
clients can set the root view of the scene, and focus views.

The scene manager also currently provides a default input pipeline implementation.

## Structured configuration

This component can be configured during product assembly via the following
configuration schema to define a set of supported input devices, which is in turn
used to determine which input handlers to assemble in the input pipeline instance:

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

The `component-for-test` target also sets a default value for testing:

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

## Domain package configuration

This component is also configured to support light sensor devices. Products may
want to provide calibration information to make use of light sensing features.
By default we provide empty calibration information, which disables light sensor
processing.

## CML file and integration tests

The production package `//src/ui/bin/scene_manager:scene_manager` includes
`meta/scene_manager.cml`, which exists to serves routes related to scene
management and input through human interface devices.

Generally, test packages should include their own copy of a component to ensure
hermeticity with respect to package loading semantics, and can do so by
including `//src/ui/bin/scene_manager:scene_manager_component_for_test`.
