# Additional logging workflows

## Use assembly overrides for logging

In some cases, you may want to use [assembly overrides][assembly-overrides-doc]
to capture the logs of a component during startup.

For example, if you want to capture the logs for the component
`my-component-logging`:

1. In your Fuchsia checkout, make a `//local/BUILD.gn` file:

   Note: For the fully list of values that you can specify, see
   [Product configuration][product-config-ref].

   ```gn
   assembly_developer_overrides("netstack-overrides") {
       platform = {
           diagnostics = {
               component_log_initial_interests = [
                   {
                       component = "core/network/netstack"
                       log_severity = "debug"
                   },
               ]
           }
       }
   }
   ```

   This is what this `BUILD.gn` file indicates:

   1. The assembly developer override is named `netstack-overrides`. You can use
      any meaningful identifier for this field.
   1. The keys `platform`, `diagnostics`, and `component_log_initial_interests`
      are all based on values from valid product configuration values. For
      more information, see [`platform`][platform-assembly-ref].
   1. In this example, you used the component moniker,
      `core/network/netstack`, but you can also use the component URL,
      `fuchsia-pkg://fuchsia.com/netstack3#meta/netstack3.cm`. For more
      information on monikers, see [component moniker].
   1. For the `log_severity` value, specify the minimum log severity to log. For
      example, `debug` severity. Alternatively, you could also use `trace` which
      is a lower severity than `debug`. However, you can use any level of log
      severity. For more information about log severity,
      see [Choosing severity for log records][log-severity-docs].

1. Once you have made a `//local/BUILD.gn` file that meets your requirements,
   you can set your `fx set` command to use this assembly platform
   configuration. For example:

   Note: The value used for `--asembly-override` is the identifier that you used
   in your `//local/BUILD.gn` file.

   ```posix-terminal
   fx set --assembly-override=//local:my-component-logging
   ```

1. Build Fuchsia:

   ```posix-terminal
   fx build
   ```

   You should now be able to run or OTA Fuchsia as you normally would.

## Restrict log severity

By default, a test will fail if it [logs][syslog] a message with a severity of
`ERROR` or higher. This often indicates that an unexpected condition had
occurred during the test, so even if the test passes it's often useful to bring
this to the developer's attention.

This default behavior can be changed, for each test package, to allow logs at
higher severities, or to fail a test that logs at lower severities. For example,
a test might *expect* to log an `ERROR`, in order to cover a failure condition and
recovery steps.

A test might expect to log at `ERROR` severity. For example, the test might be
covering a failure condition & recovery steps. Other tests might expect not to
log anything more severe than `INFO`.

For instance, to allow a test to produce `ERROR` logs:

```gn
fuchsia_component("my-package") {
  testonly = true
  manifest = "meta/my-test.cml"
  deps = [ ":my_test" ]
}

fuchsia_test_package("my-package") {
  test_specs = {
      log_settings = {
        max_severity = "ERROR"
      }
  }
  test_components = [ ":my-test" ]
}
```

To make the test fail on any message more severe than `INFO`,
set `max_severity` to `"INFO"`.

Valid values for `max_severity`: `TRACE`, `DEBUG`, `INFO`, `WARN`, `ERROR`, `FATAL`.

See also: [choosing severity for log records][choose-severity].

[choose-severity]: /docs/development/diagnostics/logs/severity.md
[syslog]: /docs/development/diagnostics/logs/README.md
[assembly-overrides-doc]: /docs/development/build/software_assembly/developer_overrides.md
[component moniker]: /docs/reference/components/moniker.md
[product-config-ref]: /reference/assembly/index.md
[log-severity-docs]: /docs/development/diagnostics/logs/severity.md
[platform-assembly-ref]: /reference/assembly/PlatformConfig/index.md
