This directory contains utilities for fuzzing the Starnix system call interface
with [syzkaller](https://github.com/google/syzkaller). The `start_sshd`
component runs `sshd` on a Starnix container, which can then be used to access
the container itself and run the Syzkaller binaries. This guide walks through
the process.

## Build and Run Starnix
Build Fuchsia with the syzkalleir_starnix package.  Add the following to your `//local/BUILD.gn` file, creating a file at that path within your Fuchsia directory if necessary.
```
import("//build/assembly/developer_overrides.gni")

assembly_developer_overrides("syzkaller_starnix") {
    testonly = true
    base_packages = [
        "//src/testing/fuzzing/syzkaller/starnix:syzkaller_starnix",
    ]
}
```
Then run the following command.
```
fx set workbench_eng.x64 --assembly-override //local:syzkaller_starnix && fx build
```
Start the Fuchsia emulator.
```
ffx emu start --headless
```
Recreate the Alpine container with the Syzkaller Starnix runner.
```
ffx component run --recreate /core/starnix_runner/playground:alpine fuchsia-pkg://fuchsia.com/syzkaller_starnix#meta/alpine_container.cm
```

## Running Syzkaller Executables
You likely wish to run syzkaller executables, for instance to reproduce a bug.
Syzkaller bugs should include a small C program to reproduce. You can compile
that small program locally, like so:
```
gcc repro.c -lpthread -static -o repro.o
```
You'll now need to copy the executable to the target container using sshd.  First start the sshd daemon:
```
ffx component run /core/starnix_runner/playground:alpine/daemons:start_sshd fuchsia-pkg://fuchsia.com/syzkaller_starnix#meta/start_sshd.cm
```
Copy your ssh public key to the Starnix container.
```
ffx component copy $(ffx config get ssh.pub | tr -d '"') /core/starnix_runner/playground:alpine::out::fs_root/tmp/authorized_keys
```
Forward the container's ssh port.

NOTE: Depending on your environment, this and the following ssh / scp commands
may require `sudo` to correctly obtain access to resources. On gLinux machines,
you may receive permission errors if you do not execute as root.
```
ssh -f -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -i $(ffx config get ssh.priv | tr -d '"')  -NT -L 12345:127.0.0.1:7000 ssh://$(ffx target list fuchsia-emulator -f a)
```
Next, you can copy the file to the container:
```
scp -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -i $(ffx config get ssh.priv | tr -d '"') -P 12345 repro.o root@localhost:/tmp
```
And access a shell within the container like so:
```
ssh -i $(ffx config get ssh.priv | tr -d '"') -p 12345 root@localhost
```
From here, you can run the executable like normal. Remember that you might want
to start an `ffx log --since now` stream in a separate terminal prior to executing.
```
cd /tmp && ./repro.o
```
