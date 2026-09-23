# Driver runner

The driver runner is the runner responsible for launching
[components][glossary.component] that run in the driver host environment.

## Using the driver runner

To use the driver runner, the component's manifest must include a `program`
block similar to the following:

```json5 {:.devsite-disable-click-to-copy}
{
    program: {
        runner: "driver",
        binary: "driver/example.so",
        bind: "meta/bind/example.bindbc",
    }
}
```

A driver component's `program` block requires the following fields at a minimum:

-   `runner` – This field must be set to the string `driver`.
-   `binary` – The path to the driver's binary output in the component's
    package.
-   `bind` – The path to the compiled bind program in the component's package.

## Optional fields

In additional to the required fields, the driver runner accepts a set of
optional fields, which are used to specify metadata or configure the runtime
environment of the driver component.

### Colocation

If the `colocate` field is set to the string `true`, the driver will be put in
the same [driver host][driver-host] as its parent driver if possible. However
this is advisory. The [driver manager][driver-manager] may still put the driver
in a separate driver host, for instance, if the parent device has `MUST_ISOLATE`
set. In DFv1, a driver is always colocated if the parent device is a composite –
isolation may still be enforced by setting `MUST_ISOLATE` on the primary
fragment of the composite.

```json5 {:.devsite-disable-click-to-copy}
{
    program: {
        runner: "driver",
        binary: "driver/example.so",
        bind: "meta/bind/example.bindbc",
        {{ '<strong>' }}colocate: "true"{{ '</strong>' }}
    }
}
```

If the `colocate` field is not specified, its value defaults to the string
`false`.

`colocate` is mutually exclusive to the [`host_restart_on_crash`](#host-restart-on-crash) field.
Only one of them can be true for a driver.

### Default dispatcher options

The `default_dispatcher_opts` field provides the options which are used when
creating the driver's [default dispatcher][driver-dispatcher], for example:

```json5 {:.devsite-disable-click-to-copy}
{
    program: {
        runner: "driver",
        binary: "driver/example.so",
        bind: "meta/bind/example.bindbc",
        {{ '<strong>' }}default_dispatcher_opts: [ "allow_sync_calls" ]{{ '</strong>' }}
    }
}
```

The options in this field correspond to the flags defined in this
[`types.h`][dispatcher-flags] file. Today, the supported options are:

-   `allow_sync_calls`: This option indicates that the dispatcher may not
    share Zircon threads with other drivers. This setting allows the driver
    to make synchronous Banjo or FIDL calls on the dispatcher without
    deadlocking.

### Default dispatcher scheduler role

The `default_dispatcher_scheduler_role` field provides the options that are used when
creating the driver's [default dispatcher][driver-dispatcher], for example:

```json5 {:.devsite-disable-click-to-copy}
{
    program: {
        runner: "driver",
        binary: "driver/example.so",
        bind: "meta/bind/example.bindbc",
        {{ '<strong>' }}default_dispatcher_scheduler_role: "fuchsia.graphics.display.driver"{{ '</strong>' }}
    }
}
```

Make sure that the scheduler roles that you specify match what a component would send through the `fuchsia.scheduler/RoleManager.SetRole` FIDL API.

### Allowed scheduler roles

The `allowed_scheduler_roles` field dictates what is allowed to be passed in as a
scheduler_role when creating new dispatchers, for example:

```json5 {:.devsite-disable-click-to-copy}
{
    program: {
        runner: "driver",
        binary: "driver/example.so",
        bind: "meta/bind/example.bindbc",
        {{ '<strong>' }}allowed_scheduler_roles: "fuchsia.graphics.display.driver"{{ '</strong>' }}
    }
}
```

This would allow the driver to create a new dispatcher at runtime and specify the
`"fuchsia.graphics.display.driver"` scheduler_role.

### VMAR scheduler role

The `memory_priority_role` field provides the options that are used when
allocating the vmar the driver is mapped into, for example:

```json5 {:.devsite-disable-click-to-copy}
{
    program: {
        runner: "driver",
        binary: "driver/example.so",
        bind: "meta/bind/example.bindbc",
        {{ '<strong>' }}memory_priority_role: "fuchsia.graphics.display.driver.vmar"{{ '</strong>' }}
    }
}
```

Make sure that the scheduler roles that you specify match what a component would send through the `fuchsia.scheduler/RoleManager.SetRole` FIDL API.

### Fallback

If the `fallback` field is set to the string `true`, this fallback driver will
only attempt to bind once all the base driver packages are indexed. Furthermore,
if this driver matches to a node and a non-fallback driver matches to the same
node, the non-fallback driver will bind to the node instead.

```json5 {:.devsite-disable-click-to-copy}
{
    program: {
        runner: "driver",
        binary: "driver/example.so",
        bind: "meta/bind/example.bindbc",
        {{ '<strong>' }}fallback: "true"{{ '</strong>' }}
    }
}
```

If the `fallback` field is not specified, its value defaults to the string
`false`.

### Next vDSO

If the `use_next_vdso` field is set to the string `true`, the driver will be put in
a [driver host][driver-host] with the next vdso dynamic linked in. The driver must
also have `colocate` set to `true` or this field is ignored.

```json5 {:.devsite-disable-click-to-copy}
{
    program: {
        runner: "driver",
        binary: "driver/example.so",
        bind: "meta/bind/example.bindbc",
        colocate: "true"
        {{ '<strong>' }}use_next_vdso: "true"{{ '</strong>' }}
    }
}
```

If the `use_next_vdso` field is not specified, its value defaults to the string
`false`.

### Device categories

The `device_categories` field provides metadata indicating the device categories
that the driver controls, for example:

```json5 {:.devsite-disable-click-to-copy}
{
    program: {
        runner: "driver",
        binary: "driver/example.so",
        bind: "meta/bind/example.bindbc",
        {{ '<strong>' }}device_categories: [
            { category: "board", subcategory: "i2c" },
            { category: "sensor", subcategory: "temperature" },
        ]{{ '</strong>' }}
    }
}
```

This metadata is used to determine the tests that the driver will undergo during
its certification process. See the full list of device categories and
subcategories in the [FHCP schema][fhcp-schema].

### Host restart on crash {:#host-restart-on-crash}

The `host_restart_on_crash` field tells the driver framework that it should restart the
driver host for the node that the driver binds to should the driver go down unexpectedly.

This includes if:

 - The driver host crashes.
 - The driver closes its client end to the `fuchsia.driver.framework/Node` protocol while running.

Because this affects the driver host, it can only be set by the root driver of the host.
The root driver is the driver for which the host was created. This is the case if and only if
the `colocate` field is set to `false`.

Therefore `host_restart_on_crash` and [`colocate`](#colocation) are mutually exclusive. Only
one of them can be `true` for a driver.

```json5 {:.devsite-disable-click-to-copy}
{
    program: {
        runner: "driver",
        binary: "driver/example.so",
        bind: "meta/bind/example.bindbc",
        {{ '<strong>' }}host_restart_on_crash: "true"{{ '</strong>' }}
    }
}
```

If the `host_restart_on_crash` field is not specified, its value defaults to the string
`false`.

When `host_restart_on_crash` is `false`, the node is removed from the driver framework's
node topology if the driver goes down unexpectedly.

### Service Connect Validation {:#service-connect-validation}

The `service_connect_validation` field is used by the driver sdk's DriverBase
to enable availability valdations to run on service capability connections.

It does this by looking through the offers available to the bound node of the
drive, and ensuring all `incoming()->Connect()` requests are trying to connect
to a valid offer.

In single parent cases, this just ensures the service is available to the node,
as all requests should be going to a `"default"` instance, when no instance is
specified by the user.

In composite cases, this ensures that the instance name requested, and the
`"default"` instance name case, have a corresponding offer from that parent.

If these validations fail, the `Connect()` method will return ZX_ERR_NOT_FOUND
immediately, instead of making a connection that will fail only when a two-way
method is called on it.

```json5 {:.devsite-disable-click-to-copy}
{
    program: {
        runner: "driver",
        binary: "driver/example.so",
        bind: "meta/bind/example.bindbc",
        {{ '<strong>' }}service_connect_validation: "true"{{ '</strong>' }}
    }
}
```

If this field is not set, the validations are disabled by default.


## Further reading

For more detailed explanation of how drivers are bound, see
[Driver binding][driver-binding].

<!-- Reference links -->

[glossary.component]: /docs/glossary/README.md#component
[driver-host]: /docs/concepts/drivers/driver_framework.md#driver_host
[driver-manager]: /docs/concepts/drivers/driver_framework.md#driver_manager
[driver-dispatcher]: /docs/concepts/drivers/driver-dispatcher-and-threads.md
[dispatcher-flags]: /sdk/lib/driver/runtime/include/lib/fdf/types.h
[fhcp-schema]: /build/drivers/FHCP.json
[driver-binding]: /docs/concepts/drivers/driver_binding.md
