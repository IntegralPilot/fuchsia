# Virtual audio utility

The virtual audio utility enables developers to interactively configure/control virtual audio
Composite devices, through the `virtual_audio` driver.

# Examples

Flags specified before `--add` modify the specification of the device that will eventually be added.

The `--add` flag creates the device according to that spec "blueprint" and adds it to devfs where it
can be detected by an audio service, tool, test or other client. Conversely, the `--remove` flag
removes a previously added device from the system. After a device is removed, it is no longer shown
in devfs; any client FIDL connections will drop in short order.

Flags specified after `--add` modify the device's behavior in realtime, such as slightly change its
clock rate, emulate a plug state change, etc.

The `--wait` option can be used to execute commands one at a time, waiting for a key press before
proceeding. This is useful for observing the types of realtime changes mentioned above.

Note: without `--wait`, the utility exits after applying the final flag, dropping all FIDL
connections (thus removing all virtual audio devices).

For example:

```
$ virtual_audio --domain=1 --add --wait --rate=1000 --wait

Executing `--domain' command...
Executing `--add' command...
Executing `--wait' command...
  Press Q to cancel, or any other key to continue...
        (user observes the device's initial clock rate, then presses a key)
Executing `--rate' command...
Executing `--wait' command...
  Press Q to cancel, or any other key to continue...
        (user observes the device's clock rate has changed, then presses a key)

$ virtual_audio --domain=1 --add --wait --unplug --wait

Executing `--domain' command...
Executing `--add' command...
Executing `--wait' command...
  Press Q to cancel, or any other key to continue...

Executing `--unplug' command...
Executing `--wait' command...
  Press Q to cancel, or any other key to continue...

$
```

