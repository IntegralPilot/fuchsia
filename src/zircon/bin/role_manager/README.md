# Role Manager

The Role Manager is a service that allows clients to
[set profiles](https://fuchsia.dev/reference/syscalls/object_set_profile?hl=en)
on threads and vmars.

## Roles

Roles are an abstraction of the scheduler profile concept using names instead
of direct parameters to avoid hard-coding scheduling parameters in the
codebase, enabling per-product customization and faster workload optimization
and performance tuning. A role is bound to a thread or vmar using `SetRole`.
A set of configuration files, including per-product configs, specifies the
mappings from role names to concrete scheduling parameters.

## Configuration Files

Profile configs are JSON file with the extension `.profiles` routed to the
`config/profiles` config directory of `RoleManager`. Any number of profile
config files may be routed, with defaults available from bringup and core
products.

The `scope` parameters in the config files determines which take precedence when
more than one file defines the same role name. The `product` scope has the
highest precedence, followed by `core`, then `bringup`, and finally none. When
there is more than one role with the same name and scope, the first one
encountered takes precedence.

The `output_parameters` configuration element can be used to specify key-value
pairs that will be returned to a caller when `SetRole` is called for a given
role.

The JSON file has the following format:

```JSON
// example.profiles
// Comments are supported. The JSON document element must be an object.
{
  // Optional scope for role overrides. Applies to all roles in the same file.
  "scope": "bringup" | "core" | "product",

  // The map of role names to scheduler parameters.
  "profiles": {
    // A profile with fair priority.
    "fuchsia.async.loop": { "priority": 16 },

    // A profile with fair priority and affinity to CPU 0 using an array of CPU numbers.
    "fuchsia.async.loop:boot-cpu": { "priority": 16, "affinity": [ 0 ] },

    // A profile with fair priority and affinity to CPUs 0 and 1 using a CPU bitmask.
    "fuchsia.async.loop:two-cpus": { "priority": 16, "affinity": 3 },

    // A profile with deadline parameters.
    "fuchsia.drivers.acme.irq-handler": { "capacity": "500us", "deadline": "1ms", "period": "1ms" },

    // A profile with deadline parameters and affinity to CPUs 2-5 using array of CPU numbers.
    "fuchsia.drivers.acme.irq-handler:bigs": {
      "capacity": "500us", "deadline": "1ms", "period": "1ms", "affinity": [ 2, 3, 4, 5 ] },

    // A profile with input and output parameters.
    "fuchsia.drivers.parameterized:param1=foo,param2=bar": { "priority": 16, "output_parameters": { "output1": "baz" } },
  }, // Trailing commas are allowed.
}
```

## Fake Role Manager

If your component uses the `fuchsia.scheduler.RoleManager` protocol and you need
to test it without using the system's actual `RoleManager` component, consider
using the `FakeRoleManager` component in the `testing/fake` directory.

`fake_role_manager.cm` is a drop-in replacement for `role_manager.cm` that
implements the `fuchsia.scheduler.RoleManager` protocol but does not actually
set scheduler profiles on the given handle in a `SetRole` call.

Note that users of this fake should provide there own profiles file that
contains the roles their component uses. This profiles file should be placed in
the `/pkg/profiles` directory.

## Test Details

The tests in the `tests` directory use a test realm
[factory](https://fuchsia.dev/fuchsia-src/development/testing/components/test_realm_factory?hl=en)
to spawn a `RoleManager` instance inside the test realm and then send requests
to it. This test realm factory can be found in the `testing/realm-factory`
directory.

Here's what the test realm looks like:

                                    -----------------
                                    |   Test Root   |
                                    -----------------
                                    /                \
                                   /                  \
                        -----------------          -----------------
                        |   Test Binary |          |   Test Realm  |
                        -----------------          |     Factory   |
                                                   -----------------
                                                           |
                                                           |
                                                   -----------------
                                                   |  Role Manager |
                                                   -----------------

The `Test Root` routes `fuchsia.scheduler.RoleManager` requests from its
`Test Binary` child to the `Role Manager` child (via the `Test Realm Factory`).

One other notable piece of this testing setup is the profile configuration
files. These can be found in the `tests/testing/realm-factory/config` directory
and are built into the `Test Realm Factory` package as `/pkg/profiles`. This
directory is then routed to the `Role Manager` component as `/config/profiles`.

## ProfileProvider

The `RoleManager` binary currently also implements the
`fuchsia.scheduler.deprecated.ProfileProvider` protocol because netstack2 still
uses that protocol. Once netstack2 has been deprecated, we can remove this.
