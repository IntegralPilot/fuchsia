<!-- Generated with `fx rfc` -->
<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0249" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

<!-- This should begin with an H2 element (for example, ## Summary).-->

## Summary

This document proposes the addition of crosvm as a supported system
configuration by the Fuchsia project. This would enable us to continuously test
Fuchsia on crosvm, and codifies our commitment to keep crosvm working at ToT.


## Motivation

As per its official documentation, [crosvm] is a hosted (a.k.a. type-2) virtual
machine monitor similar to QEMU-KVM or VirtualBox. It supports multiple
architectures including x86_64 and arm64, as well a number of backends for
hardware acceleration including Linux KVM.

There are users of Fuchsia which already employ the use of Fuchsia in crosvm
based emulator, and we expect more over time. crosvm support will enable us to
test out a number of drivers against the crosvm implementation of those
hardware. While we already have support for QEMU, FEMU, machina, and GCE for a
variety of implementations of various virtualized devices, there are quirks in
every implementation and software working in any one of these emulator
environments doesn't ensure it will work in another. In order to ensure we
don't break users of crosvm, we need to enable automated testing of Fuchsia
against crosvm in Fuchsia's testing infrastructure.

There are also some devices which crosvm provides support for which other
emulation environments we do not yet support.

Additionally, we need to make it easier for our driver developers to write and
maintain drivers which are used in a crosvm based environment. Currently this
requires running Fuchsia in an out of tree product oriented environment. This
approach does not scale.

Lastly, some technologies, such as the 64 bit Linux boot protocol for x86 is not
otherwise supported by any of our existing hardware targets. As a result, we
have yet to implement a Linux boot shim with such support yet, as it can not be
tested.

## Stakeholders

_Facilitator:_

neelsa@google.com

_Reviewers:_

- cja@google.com
- jamesr@google.com
- mcgrathr@google.com
- olivernewman@google.com
- wittrock@google.com

_Consulted:_

List people who should review the RFC, but whose approval is not required.

- cpu@google.com
- costan@google.com
- diserovich@google.com
- wilkinsonclay@google.com

_Socialization:_

This RFC went through a design review with the Engprod team. Additionally, all
major stakeholders were reached out to during the investigation efforts.

## Requirements

- We must be confident that we will not break Fuchsia customers using
  crosvm-based emulators.
- We must support both the x86_64 as well as aarch64 architectures in crosvm.
 - We must support the 64 bit linux boot protocol on x86_64 as well as aarch64.
- We must support the Linux KVM backend for crosvm. Other backends would be a
  nice to have.
- Driver developers should have an easy time testing their drivers against the
  crosvm based implementation of hardware their drivers are written for.

## Design

We will add support for the crosvm to the platform by integrating it into the
Fuchsia checkout as a CIPD prebuilt, adding support for launching crosvm to ffx,
and then finally setting up relevant bots to run in pre and/or post submit.

## Implementation

### Bringup and 64 bit Linux boot shim

In order to bootstrap support for crosvm, we need to first enable Fuchsia to
boot on crosvm. crosvm currently only supports either providing a custom
bootloader or the 64 bit Linux boot protocol. Fuchsia supports providing an EFI
based bootloader as well as multiboot and the 32 bit Linux boot protocol
currently. This non-intersecting set of boot options prevents Fuchsia from
currently booting on crosvm. So the first step in order to support crosvm is to
conduct a bringup activity for crosvm and implement another boot shim for the 64
bit boot protocol.

It is noteworthy to mention that this problem only exists on x86. Fuchsia does
boot on crosvm under aarch64. However, because crosvm only supports hardware
accelerated virtualization by default, it is difficult for the average Fuchsia
developer to actually run Fuchsia on crosvm under aarch64. While we have proven
that crosvm was able to boot Fuchsia using the arm64 Linux boot shim in the
past, there may be additional work necessary there as well to ensure it
continues to boot.

Beyond the boot shim work, it's possible some work will be necessary for
relevant storage and networking drivers to ensure the device works with ffx and
infrastructure. This work will be uncovered after we finish bootshim work, which
will land imminently for x86_64. We may require support from the crosvm team to
get any remaining necessary fixes landed in their code.

### crosvm prebuilt roller

A new builder will be set up to pull the latest sources from the upstream crosvm
repository, build it with relevant features enabled, and then upload the final
artifact to CIPD.

A roller will be responsible for updating the integration repository to ensure
future `jiri update` commands result in the updated crosvm binary be delivered.

### ffx emu support for crosvm as an alternative engine

An additional option will be added to the `ffx emu start --engine` flag to
enable it to choose crosvm instead of qemu or femu. The particular set of
devices that `ffx emu` will present to the user by default will be aligned to be
as close to real out of tree use cases of crosvm as possible. Because there are
multiple users of crosvm, this might mean we need to support multiple device
configurations.

An optional `fx crosvm` command wrapper may be added to simplify this workflow
further, though we already have some support in `fx run-boot-test --crosvm`.

### CI/CQ Builders

Once the platform has the ability to launch Fuchsia on crosvm, we will introduce
builders which will ensure that crosvm support continues to work as expected.

For our purposes, because the minimal.x64 product/board combination will work
with crosvm, we will not need another builder as we can repurpose the builder
the same builders we use for qemu.

Initially these builders will be FYI builders, but as they prove themselves to
be stable, we can update them to be blocking, as is the typical process for
creating new blocking builders.

Whether these builders run in CI or CQ, and how many builders are created in
total is beyond the scope of this proposal.

We should additionally attempt to get bringup.x64 builders to run their boot
tests on crosvm, as it would minimize the surface area of potential boot-time
bugs.

#### Tests

The tests we must run in CI and/or CQ will be limited to the following use
cases:

* Reboot and shutdown tests
* Zircon core tests
* Device enumeration tests which ensure all peripheral hardware enumerates
  correctly.
* Systems tests which ensure the peripheral drivers operate as expected.

Notably, hermetic tests which currently run on QEMU do not need to be run
duplicatively on crosvm.

For the most part, we can accomplish this test segmentation by relying on
specifying a new environment in the tests_specs which appear on tests.

Reboot and shutdown tests currently run as host tests and have a hard dependency
on qemu which we will need to address. Additionally, these tests are not capable
of being run on arm64 bots, so we will also need to remedy that problem to
ensure proper test coverage.

### Additional peripheral bringup

Beyond the set of devices necessary to enable CI/CQ bots, there are additional
peripherals we will want to invest into in order to support our customer use
cases. The complete set of these peripherals is beyond the scope of this
proposal. Some possible peripherals that may be implemented include:

* virtio-gpu
* virtio-balloon
* cmos-rtc

## Performance

We should ensure that beyond functional, Fuchsia is performant on crosvm. If
boot speed is slow or our virtio drivers take up too much CPU when under load,
we will need to do some analysis and optimization work to ensure we meet the
performance needs of our customers using crosvm.

## Ergonomics

This proposal will allow driver developers to more easily test their drivers
against the crosvm implementation of the hardware their drivers target.


## Backwards Compatibility

Not applicable.

## Security considerations

Not applicable.

## Privacy considerations

Not applicable.

## Testing

The purpose of this propsal is to add crosvm support to the platform and add
relevant automated test coverage to CI/CQ.

## Documentation

Documentation for ffx emu should be updated to reflect new support for the
crosvm engine. Additionally, a new page for supported emulators should be
created to describe newfound support for crosvm.

## Drawbacks, alternatives, and unknowns

### Alternative: Do nothing and continue to use existing emulators as a proxy for support

The easiest option is to do nothing and rely on qemu and/or femu to help ensure
our virtio drivers continue to work as intended, manually testing on crosvm as
needed. This option would give us very little confidence that our crosvm support
wouldn't break OOT users, but breakages could be rare given existing emulators
should share majority of behavior as crosvm.

### Alternative: Try and get crosvm to officially support Fuchsia instead

Other operating systems don't go out of their way to support crosvm. Instead,
crosvm tries to support those operating systems directly instead, by matching
its behavior to existing drivers in those operating systems. We could possibly
petition crosvm to support Fuchsia in the same way. Ultimately though, Fuchsia
is a much more niche target than other major operating systems, so it would be
challenging to actually convince crosvm to take such support upon themselves.

Our experience with vendor booting firmware (even open source firmware, just as
CrOS depthcharge) in general tells us that it can be challenging to get
Fuchsia-specific booting support implemented and maintained robustly.

Another downside of this is that it doesn't give us crosvm's Linux booting
support as another case of diversity of entrenched booting scenarios that we can
now test and ensure our compatibility with. Every additional variation of the
Linux/x86 and Linux/arm64 boot protocols we can support makes future bringup on
additional sets of boards with stock firmware more likely to be smooth and
closer to out-of-the-box, and likewise for the long-term "general x86 PC" case
that is historically far and away the most likely vehicle for wider public
adoption of a new open source OS.

### Unknowns

There are many unknowns in bringing up a new emulator, including the amount of
engineering time we'll need to spend to debug and work around issues we don't
currently know about. Bugs we find may require the kernel, drivers, or other
teams to put in additional, previously unbudgeted work, or we'll need to
reprioritize this effort. If and when we find bugs, we'll prioritize with the
relevant groups.

## Prior art and references

Not applicable.

[crosvm]: https://crosvm.dev/book/
