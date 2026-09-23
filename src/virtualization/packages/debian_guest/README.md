# Debian Guest

The `debian_guest` package provides a basic Linux environment based on the
Debian Linux distribution.

## Building

These steps will walk through building a package with the root filesystem
bundled as a package resource. The root filesystem will appear writable but
all writes are volatile and will disappear when the guest shuts down.

```
$ cd $FUCHSIA_DIR
$ ./src/virtualization/packages/debian_guest/build-image.sh prebuilt/virtualization/packages/debian_guest/images/x64 x64
$ fx set core.x64 --with-base "//src/virtualization,//src/virtualization/packages/debian_guest"
$ fx build
$ fx pave
```

To boot on an ARM64 device, replace `x64` with `arm64`.

## Running `debian_guest`

Once booted:

```
guest launch debian
```

## Telnet shell

The Debian system exposes a simple telnet interface over vsock port 23. You can
use the `guest` CLI to connect to this socket to open a shell. First we need to
identify the environment ID and the guest context ID (CID) to use:

```
$ guest list
env:0             debian
 guest:3          debian
```

The above indicates the debian guest is CID 3 in environment 0. Open a shell
with:

```
$ guest socat 0 3 23
```

## CIPD (Googlers only)

CIPD images for the Debian guest are available to Googlers, and updateable by
anyone in the [fuchsia-cipd-linux-debian-eligible](https://ganpati2.corp.google.com/group/101332328874)
group. If you are not in that group, you can file a bug and assign it to someone
in the group. The recommended assignee is aknobloch@.

**For easier auditing and rollbacks, it's requested to land all changes to the
build scripts and tests alongside the changes to the pinned CIPD versions.** As
an example of such a change, see fxr/1740270.

For those wishing to update the images, you should first make the relevant
updates to the images. The most likely place you'll want to modify is
`debos/config_rootfs.sh` for adjustments to the included kernel modules or other
initialization procedures. You'll also likely need to make test changes.

After your changes are made, you'll run `build-image.sh` to create artifacts for
x64 and arm64 image targets. Note that on gLinux hosts, this process can be
frustratingly flakey. The fakemachine build process is slow, and will frequently
fail. You can disable fakemachine when targeting an architecture that's the same
as your host (e.g. building x64 images on an x64 host). When fakemachine is
necessary, for example building arm64 images on an x64 host, you may need to
retry the build a few times before it's successful.

Here are some example build commands that you might find useful:
```
# Building x64 with a sideloaded Linux 6.6 kernel. Note that debos requires sudo
# permissions when running on the host directly:
sudo src/virtualization/packages/debian_guest/build-image.sh /tmp/6.6_debian_x64/ x64 --disable-fakemachine --sideload-kernel https://snapshot.debian.org/archive/debian/20240125T031154Z/pool/main/l/linux-signed-amd64/linux-image-6.6.13-amd64_6.6.13-1_amd64.deb

# Building arm64, using fakemachine, with a sideloaded Linux 6.6 kernel:
src/virtualization/packages/debian_guest/build-image.sh /tmp/6.6_debian_arm64 arm64 --sideload-kernel https://snapshot.debian.org/archive/debian/20240125T031154Z/pool/main/l/linux-signed-arm64/linux-image-6.6.13-arm64_6.6.13-1_arm64.deb
```

You can test the images locally by overwriting prebuilt artifacts in your BUILD.
For example, assuming you are on an x64 host:
```
# Note that overwriting prebuilt artifacts requires elevated permissions.
sudo cp /tmp/6.6_debian_x64/* prebuilt/virtualization/packages/debian_guest/images/x64/
```

After overwriting the prebuilt artifacts, you'll need to rebuild and test. Once
you are satisfied that the updated artifacts work, request a temporary grant to
update artifacts and upload them to CIPD directly:
```
# Request a temporary grant, adding your bug and justification reason:
grants add fuchsia-cipd-linux-debian-acl --reason="b/YOUR_BUG -- Your justification"
```

NOTE: It's been observed that grants can quite a while (over an hour) to roll
through the system. If you encounter denials after adding a temporary grant,
try extending the grant time (`grants add -t 20h0m0s`) and waiting a while.

After your membership grant takes affect, upload the artifacts to CIPD:
```
# Upload both ARM64 and AMD64 artifacts:
cipd create -in /tmp/6.6_debian_x64/ -name fuchsia_internal/linux/debian_guest-x64 -install-mode copy
cipd create -in /tmp/6.6_debian_arm64/ -name fuchsia_internal/linux/debian_guest-arm64 -install-mode copy
```

You should see output similar to the following:
```
Instance fuchsia_internal/linux/debian_guest-x64:<unique_instance_id> was successfully registered
```

The final step will be to take the instance IDs emitted from the create command
and update the pinned CIPD instance in `manifests/prebuilt`. Once again, it's
recommended to update these prebuilt IDs alongside any functional or testing
changes you made to the build scripts and test suites. This makes it easier to
audit changes, as well as atomically roll them back if required. You can find an
example of image updates and versioning in the following CL:

https://fuchsia-review.git.corp.google.com/c/fuchsia/+/1740270
