# Feedback

For development, it is often easier to dump the crash information in the logs as
the crash happens on device. For devices in the field, reports may be sent to a
remote crash server, as access to the devices' logs might not be available.
The `fuchsia.net.http.Loader` protocol is used to talk to the remote crash
server. Whether reports are uploaded is controlled via
[Assembly](https://fuchsia.dev/fuchsia-src/concepts/software_assembly/overview).

## Testing

To test your changes, on a real device, we have some unit tests and some helper
programs to simulate various crashes. For the helper programs, you first need to
apply a developer override to enable uploads on an eng device. For example, the
userdebug config enables crash report uploading if the privacy settings also
have it enabled.

### Developer overrides

First, define one or more developer overrides in `//local/BUILD.gn`. This file
is Git-ignored so you can add it once and only use/modify as needed:

```
import("//build/assembly/developer_overrides.gni")

assembly_developer_overrides("feedback_userdebug_config") {
  developer_only_options = {
    forensics_options = {
      build_type_override = "userdebug"
    }
  }
}
```

You can apply an override via `fx set` using a label from `//local/BUILD.gn`,
such as:
`product_assembly_overrides_label = "//local:feedback_userdebug_config"`.

Alternatively, overrides can be specified in-line in args.gn:

```
product_assembly_overrides_contents = {
  developer_only_options = {
    forensics_options = {
      build_type_override = "userdebug"
    }
  }
}
```

Valid options for `build_type_override` are `user`, `userdebug` and
`eng_with_upload`.

Note, the above overrides only work for eng builds and product assembly will fail
if used on user/userdebug builds.

Then, after running each one of the helper programs (see commands in sections
below), you should then look each time for the following line in the syslog:

```sh
(host)$ ffx log --tag forensics
...
Successfully uploaded report at $URL...
...
```

Click on the URL (contact OWNERS if you don't have access and think you should)
and check that the report matches your expectations, e.g., the new annotation is
set to the expected value.

### Unit tests

To run the unit and integration tests:

```sh
(host) $ fx test //src/developer/forensics
```

### Root job termination

The following command causes the root job to be terminated due to a critical
process death:

```sh
(host)$ ffx component stop archivist.cm
```

The device will then reboot and the system should detect the kernel crash log,
attach it to a crash report and try to upload it. Look at the syslog upon
reboot.

### C userspace crash

The following command will cause a write to address 0x0:

```sh
(target)$ crasher
```

You can immediately look at the syslog.

## Question? Bug? Feature request?

Contact OWNERS.
