# Download the Fuchsia IDK

You can download the Fuchsia Integrator Developer Kit (IDK) using the links below.
Please be aware that Fuchsia is under active development and its API surface is
subject to frequent changes. The Fuchsia IDK is produced continuously as Fuchsia
is developed.

Because the [Fuchsia System Interface](/docs/concepts/kernel/system.md) is
changing, you will need to run software built using a particular version of
the IDK on a Fuchsia system with a matching version. The [IDK](#core) contains
a matching system image appropriate for running in the
[QEMU](https://www.qemu.org/){:.external} emulator.

## Integrator Developer Kit {#core}

The Integrator Developer Kit (IDK) is independent of any specific build system
or development environment. The IDK contains metadata that can be used by an
[IDK backend](README.md#backend) to generate an SDK for a specific build system.

* [Linux](https://chrome-infra-packages.appspot.com/p/fuchsia/sdk/core/linux-amd64/+/latest)

