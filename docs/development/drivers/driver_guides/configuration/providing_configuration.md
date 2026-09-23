# Providing configuration

If you are using an existing configuration that other drivers are already using,
no extra steps are needed.

For example for `fuchsia.power.SuspendEnabled`, this configuration is
provided through the [power subsystem assembly][power-assembly].

## Adding a new configuration

To add a new configuration, identify which platform subsystem it should belong
to by looking in the [subsystems directory][subsys]. If the subsystem
already exists, then add it in that subsystem.
Otherwise discuss your use case with the
[Fuchsia Assembly team][assembly-owners] to find the best location.

```rust
builder.set_config_capability(
    "fuchsia.gizmo.Configuration",
    Config::new(ConfigValueType::Bool, true),
)?;
```

### Routing a new configuration

Route this configuration into the driver collections.

1.  Route the configuration to bootstrap in the [Root component][root-manifest].
The route should be:

    ```json5
    from: "#config",
    to: "#bootstrap",
    ```

1.  Route the configuration in the various bootstrap shards for drivers.
The route should be `from: "parent"` and:

    *   [Boot drivers shard][boot-shard] `to: "#boot-drivers",`
    *   [Base drivers shard][base-shard] `to: "#base-drivers",`


[assembly-owners]: /src/lib/assembly/OWNERS
[subsys]: /src/lib/assembly/platform_configuration/src/subsystems/
[root-manifest]: /src/sys/root/root.cml
[boot-shard]: /src/devices/bin/driver_framework/meta/driver_framework.bootstrap_shard.cml
[base-shard]: /src/devices/bin/driver_framework/meta/base_drivers.bootstrap_shard.cml
[power-assembly]: /src/lib/assembly/platform_configuration/src/subsystems/power.rs