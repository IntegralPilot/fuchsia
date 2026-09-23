# Blackout - Power Failure Tests for the Filesystems

The blackout tests are designed to provide confidence in the ability for our filesystems to stay
consistent when real world power events happen.

## Consistency - what does it mean?

The consistency guarantee this is testing means that the filesystem won't become corrupt if it is
interrupted mid-operation. We don't make any particular guarantees about user data - unless there
is a flush, it may or may not be written, in any order. However, the filesystem metadata _must_
continue to have a self-consistent view of the world.

To verify this, we run fsck, which does a large variety of consistency checks on the filesystem
metadata.

## Overview of a blackout test

All blackout tests follow the same pattern:

 - Find and set up a partition for testing
 - Run a load generation routine against the filesystem
 - Interrupt that load generation. The tests support the following modes -
   - "None" - no reboot between load gen and verification. In this case, the test should have an
     end condition so it exits.
   - "Soft-Reboot" - runs `power reboot` on the device.
   - "Hard-Reboot" - in infra, this will use the DMC to issue a power cycle to the device harness.
   - "Suspend-Resume" - in progress.
 - Verify that the filesystem is consistent. This likely runs fsck.

The host-side harness, in `host/blackout.py`, uses the lacewing framework, and can run any blackout
test. It takes a set of configuration options.

 - `component_name` - the name blackout should create the component with. This likely includes the
   dynamic component group name, e.g. `//core/ffx-laboratory:blackout-target`
 - `component_url` - the actual test component to run. This test component should implement the
   `fuchsia.blackout.test.Controller` interface, and something has to include it in the build
   graph.
 - `device_label` (optional) - the label of the block device the component should run its test on.
   This is passed directly to the component. The test components will normally try to find this
   partition, and create it in a gpt if it doesn't exist already, but it's up to the specific test.
   If it's not provided, `default-test` is used.
 - `device_path` (optional) - the path of the block device to use for the test. This is optional,
   and it's optional in the Controller protocol too, so a None will be passed to the component if
   it's not provided. Like the label, it's up to the test to decide what should be done with it,
   but normally if it's provided, tests will ignore the label and use this device directly.
 - `load_generation_duration` (optional) - How long the load generation step should block before
   returning. If the value is zero, it will block until the test logic returns. This doesn't
   necessarily cancel the load generation or shut down the filesystem.
 - `test_case_revive` - This should always be `true` for blackout tests.
 - `fuchsia_device_operation` - This string describes the reboot strategy. See above for possible
   values and what they mean.

## Running a blackout test locally

You can run blackout tests locally against an emulator or a device using `fx test --e2e <name>`.
Lacewing should automatically connect to your fuchsia device. If the device has a gpt, fxfs-tree
will automatically create the partition, so that's all you need to do. The integration tests don't
need a partition so they will also work out of the box.

### Running fxfs-tree on an emulator

For the fxfs-tree test, which runs load generation against fxfs, the test needs a partition to run
against. On an emulator, there are two ways to do this. First option, you can modify the
`python_mobly_test` target for the test (or add a new one) for local testing, changing the
`device_path` argument. Second option, if you set up a gpt on the device, it should create a
partition in the gpt automatically.

For either option, you need to add a new disk to the emulator. First we create a new file to act as
our disk -

```
touch blk.bin
truncate -s 256M blk.bin
# If you want to set up a gpt
fdisk blk.bin  # Press 'g' to create a GPT partition table, and then 'w' to save
```

If you use `fx qemu`, you can add the arguments for attaching the disk straight to the command
line.

```
fx qemu -kN  -- -drive file=$PWD/blk.bin,index=0,media=disk,cache=directsync,format=raw
```

If you use `ffx emu start`, you need to put that qemu argument into a json file that gets added to
the qemu invocation, and pass it to emu with the `--dev-config` argument.

```json
{
    "args": [
        "-drive",
        "file=/path/to/blk.bin,index=0,media=disk,cache=directsync,format=raw"
    ]
}
```

```
ffx emu start --headless --console --net tap --dev-config ~/blackout-shard.json
```
