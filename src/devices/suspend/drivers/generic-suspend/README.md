# Generic Suspend HAL

## Building

To add this component to your build, append
`--with-base //src/devices/suspend/drivers/generic-suspend`
to your `fx set` invocation.

## Testing

To add tests to your build, append `--with
//src/devices/suspend/drivers:tests` to your `fx set` invocation.

Run tests with:

```
$ fx test generic-suspend-test
```

To see output, use the `-o` flag.

## Inspect

To grab the inspect data, run this `ffx` command:

```
$ ffx inspect show "bootstrap/boot-drivers\:suspend"
bootstrap/boot-drivers:suspend:
  metadata:
    name = generic-suspend
    component_url = fuchsia-boot:///generic-suspend#meta/generic-suspend.cm
    timestamp = 85237566250
  payload:
    root:
      suspend_events:
        0:
          suspended = 73550318125
        1:
          resumed = 78570003416
```

The suspend and resume times are listed in chronological order. A suspend abort
will be annotated as `suspend_failed` with the failure time.
