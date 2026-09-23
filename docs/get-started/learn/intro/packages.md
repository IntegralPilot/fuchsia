# Software delivery

<<../../_common/intro/_packages_intro.md>>

<<../../_common/intro/_packages_serving.md>>

<<../../_common/intro/_packages_storing.md>>

## Exercise: Packages

So far in this codelab, you've been experiencing on demand software delivery
to your device and you probably didn't even know it! In this exercise, you'll
peel back the covers and see the details of how packages are delivered and stored
on a Fuchsia device.

<<../_common/_restart_femu.md>>

### Start a local package server

Run the following command to start a package server and enable the emulator to
load software packages:

```posix-terminal
fx serve
```

The command prints output similar to the following, indicating the server is
running and has successfully registered the emulator as a target device:

```none {:.devsite-disable-click-to-copy}
[serve] Discovery...
[serve] Device up
[serve] Registering devhost as update source
[serve] Ready to push packages!
[serve] Target uptime: 139
[pm auto] adding client: [fe80::5888:cea3:7557:7384%qemu]:46126
[pm auto] client count: 1
```

### Examine the package server

The `fx serve` command runs a **local package server** used to deliver
packages to the target devices. By default, this server runs at on port 8083.

Open a browser to `http://localhost:8083`. This loads an HTML page listing all
the packages currently available in the package repository. Each one of these
are packages that can be delivered to the device.

### Monitor package loading

Packages are resolved and loaded on demand by a Fuchsia device. Take a look at
this in action with the `spinning-square` example package.

From the device shell prompt, you can confirm whether a known package is
currently on the device:

```posix-terminal
fx shell pkgctl pkg-status fuchsia-pkg://fuchsia.com/spinning-square-rs
```

```none {:.devsite-disable-click-to-copy}
Package in registered TUF repo: yes (merkle=ef65e2ed...)
Package on disk: no
```

Open a new terminal and begin streaming the device logs for `pkg-resolver`:

```posix-terminal
ffx log --filter pkg-resolver
```

This shows all the instances where a package was loaded from the package
server.

From the device shell prompt, attempt to resolve the package:

```posix-terminal
ffx target package resolve fuchsia-pkg://fuchsia.com/spinning-square-rs
```

Note: If you work on an older version of Fuchsia, `ffx target package` might not
be available, and the above command will error out. If this is the case, use
the following as a fallback:
`fx shell pkgctl resolve fuchsia-pkg://fuchsia.com/spinning-square-rs`

Notice the new lines added to the log output for `pkg-resolver`:

```none {:.devsite-disable-click-to-copy}
[pkg-resolver] INFO: attempting to resolve fuchsia-pkg://fuchsia.com/spinning-square-rs as fuchsia-pkg://default/spinning-square-rs with TUF
[pkg-resolver] INFO: resolved fuchsia-pkg://fuchsia.com/spinning-square-rs as fuchsia-pkg://default/spinning-square-rs to 21967ecc643257800b8ca14420c7f023c1ede7a76068da5faedf328f9d9d3649 with TUF
```

From the device shell prompt, check the package status again on the device:

```posix-terminal
fx shell pkgctl pkg-status fuchsia-pkg://fuchsia.com/spinning-square-rs
```

```none {:.devsite-disable-click-to-copy}
Package in registered TUF repo: yes (merkle=21967ecc...)
Package on disk: yes
```

Fuchsia resolved the package and loaded it from the local TUF repository on
demand!

### Explore package metadata

Now that the `spinning-square` package has successfully been resolved, you can
explore the package contents. Once resolved, the package is referenced on the
target device using its content address.

From the device shell prompt, use the `pkgctl get-hash` command to determine the
package hash for `spinning-square`:

```posix-terminal
fx shell pkgctl get-hash fuchsia-pkg://fuchsia.com/spinning-square-rs
```

The command returns the unique package hash:

```none {:.devsite-disable-click-to-copy}
ef65e2ed...
```

Provide the full package hash to the `pkgctl open` command to view the package
contents:

```posix-terminal
fx shell pkgctl open {{ '<var>' }}ef65e2ed...{{ '</var>' }}
```

```none {:.devsite-disable-click-to-copy}
opening ef65e2ed...
package contents:
/bin/spinning_square
/lib/VkLayer_khronos_validation.so
/lib/ld.so.1
/lib/libasync-default.so
/lib/libbackend_fuchsia_globals.so
/lib/libc++.so.2
/lib/libc++abi.so.1
/lib/libfdio.so
/lib/librust-trace-provider.so
/lib/libstd-e3c06c8874beb723.so
/lib/libsyslog.so
/lib/libtrace-engine.so
/lib/libunwind.so.1
/lib/libvulkan.so
/meta/contents
/meta/package
/meta/spinning-square-rs.cm
/data/fonts/RobotoSlab-Regular.ttf
/meta/fuchsia.abi/abi-revision
/data/vulkan/explicit_layer.d/VkLayer_khronos_validation.json
```

This lists the package metadata and each of the content BLOBs in the package.
You can see `bin/` entries for executables, `lib/` entries for shared library
dependencies, additional metadata and resources.

## What's Next?

Congratulations! You now have a better understanding of what makes Fuchsia
unique and the goals driving this new platform's design.

In the next module, you'll learn more about the Fuchsia open source project and
the tools used to build and customize the system:

<a class="button button-primary"
    href="/docs/get-started/learn/build">Building Fuchsia</a>
