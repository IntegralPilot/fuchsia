<!-- Generated with fx rfc -->
<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0222" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}

<!-- mdformat on -->

## Summary

The Future Fuchsia Experience tool, [FFX][ffx-reference], provides a method for
communicating with a Fuchsia device from a host machine. This uses the _Fuchsia
Interface Definition Language_, FIDL, to call into various services for
development and testing. In order to add more functionality to the tool, FFX
supports sub-tools, which are separate binaries for extending FFX's command-line
surface and functionality. The tooling is written entirely in Rust.

If a user wishes to access FIDL protocols on the Fuchsia device, they can either
write a plugin for FFX, use the _Scripting Layer For Fuchsia_,
[SL4F][sl4f-overview], or commit to writing a component on the system.

None of these options are ideal for lightweight experimentation, rapid
iteration, or flexibility. FFX plugins require writing tooling in Rust and
compiling it. SL4F is not only unsupported, it also relies on insecure
transport, and interfacing with it requires writing a facade, in Rust, for each
FIDL interface you plan on calling into.

None of these approaches allow users to adhere to the principle of "Bring Your
Own Runtime."

This RFC proposes an additional method to accessing the device, through existing
libraries from within FFX, which is called Fuchsia Controller. This consists of:

- A stable ABI to interact with Fuchsia Devices through FIDL handle emulation
  without requiring changes to FFX itself.
- First class support for at least one popular scripting language leveraging
  said stable ABI (current choice being Python).
- Higher level libraries in said scripting language optimized for writing
  critical testing use cases.


## Motivation

There is currently no supported mechanism for scripting interactions from a host
device to a Fuchsia device (via the SDK, in-tree, or otherwise).

If the user wishes to use FFX for interacting with a Fuchsia device, they must
have the definition of a service available to be compiled into the tool. If
there is a service defined outside of the tree, users cannot access it. If the
service is already defined, then the user must write an FFX plugin and compile
it.

The Fuchsia Controller would allow users to interact with a Fuchsia Device in a
way that is rapid, highly iterative, and easy to automate. Supporting a stable
ABI will allow users to Bring Their Own Runtime should they so choose to
implement it, as using a C ABI is supported for extending almost all popular
programming languages.

## Stakeholders

_Facilitator:_

TBD

_Reviewers:_

jeremymanson@google.com (Tools)
mgnb@google.com (FFX)
ianloic@google.com (FIDL)
chok@google.com (lacewing)
jzgriffin@google.com (SL4F)

_Consulted:_

EngProd team
FFX team
FIDL team

_Socialization:_

Fuchsia Controller is the product of several months of conversation between
Tools, FFX, FIDL, and EngProd teams. Other teams include the Component Framework
and Test Architecture teams. The author has also written and demonstrated a
[proof of concept][fuchsia-controller-prototype] and high level demo using the
techniques outlined in the implementation section.

## Design

### Overview

The Fuchsia Controller consists of three parts:

* The FFX client library.
* The language-specific extension library.
* The higher level first class libraries using the extension library.

These all work in conjunction with FFX in order to handle connecting to a
Fuchsia device.

![Alt_text: A diagram of the Fuchsia Controller. The Fuchsia Controller stack
starts at higher level python, which feeds into the main python bindings, then
into FFX bindings. From here FIDL is passed through to a Unix Domain Socket that
the FFX daemon monitors. From here the FFX daemon uses a tunneled FIDL
connection to the Fuchsia Device to interact. This example outlines a general
interaction, namely getting a proxy to a component that the Remote Control
Service
exposes](resources/0222_fuchsia_controller/fuchsia_controller_diagram.png)

Before continuing there are some requisite things to cover.

### Background of FFX

FFX is a host-side tool that (at the time of writing) uses a "Daemon," which in
this case is a forked copy of itself running in the background, that actively
connects to Fuchsia devices via SSH as they are discovered on the network.
There are numerous means for discovering devices but they are beyond the purview
of this document.

The FFX Daemon allows for users of the FFX tool to open emulated FIDL Channels
between the host and a Fuchsia device. Messages are routed from the FFX
Client invocation, through the Daemon, then to the Fuchsia device.

FFX is written almost entirely in Rust. In addition to this, the libraries for
writing to, and reading from, host-side emulated FIDL channels are also written
in Rust.

To summarize, FFX allows the user to interface with the Fuchsia device via FIDL,
using Rust-written plugins that are tightly coupled to the implementation of
FFX.

### Defining Terms

For the following sections, some definitions:

* "Isolation" refers to using FFX in a way that it may have a separate instance
  of the FFX daemon that runs on its own, without using the common socket
  location, or any of the common FFX configuration values. The instances of the
  FFX daemon are not isolated from communicating with Fuchsia devices (unless
  invoked to do so explicitly).
* "Daemon Protocol" refers to a host-side FIDL protocol that the FFX daemon
  handles. These vary from package serving, to examining Fuchsia devices on the
  network, to Fuchsia device port forwarding. The daemon itself can be viewed as
  similar to a component that exposes several of these daemon protocols.
* When mentioning "zircon" for channels, handles, and sockets, these are
  emulated host-side via overnet. It is possible in the future that these will
  be usable on Fuchsia devices as actual zircon objects, though.

### The FFX Client Library

The FFX client library exposes, via C ABI, functions to support the
following:

* Creating, reading from, writing to, and closing zircon channels.
* Creating, reading from, writing to, and closing zircon sockets.
* Signaling zircon handles (primarily used for event pairs).
* Connecting zircon channels to components on Fuchsia devices.
* Connecting zircon channels to FFX Daemon Protocols.

There will also be support for additional functionality like:

* Creating an isolated instance of the FFX Daemon.
* Declaring FFX configuration key/value pairs.

### The Language Specific Extension

This involves a simple abstraction on top of the FFX client library,
implemented as a shared library, that will then be used in a first-class
scripting library.

## Implementation

As described previously, this will involve creating two shared libraries.

* The language agnostic "FFX client" library, which will expose functions that
  communicate with FFX as well as zircon handles.
* The language binding (in this case, Python) shared library.

In addition there will be the Python bindings for ergonomic FIDL usage.

Implementation does not involve any third party dependencies that are not
already included in the Fuchsia source tree. We expect it to be possible to
implement support for other languages using the FFX client library, which we
may support (and may require additional third party dependency work), but this
is outside the scope of this RFC.

Distributing Python along with the SDK is a separate topic out of scope of this
document, and will likely be encapsulated in its own RFC.

### The FFX Client Library

The C definitions in this section are intended to be placed in a header file
that will be delivered with the Fuchsia SDK. Due to the lack of namespaces in C
these function names will be rather verbose. The function names
are also not necessarily intended to be final.

The pattern of the ABI is intended to be such that the majority of functions
return structures for the caller using output parameters, with the return value
being either void or an error.

When possible established types like `zx_status_t` and `zx_handle_t` will be
used for congruence with existing C libraries like in the Zircon codebase.

An example function for reading channels could look like the following (this
is not intended to be a finalized implementation or a source of documentation):

```c
extern zx_status_t ffx_channel_read(void* ctx,
                                    zx_handle_t handle,
                                    void* buf,
                                    uint32_t buf_len,
                                    zx_handle_t* handles,
                                    uint32_t handles_len
                                    uint32_t* actual_bytes_count,
                                    uint32_t* actual_handles_count);
```

The `ctx` value could point to a Rust object containing information about an
existing FFX environment (running directory, location of the SDK, configuration
values, and the Fuchsia device with which we're communicating).

This is analogous to the existing [zx_channel_read][zx-channel-read], only
omitting the option parameter. Rather than exporting the same symbol as the
Zircon syscall the ABI will be explicitly written in the header so it is clear
what functionality is and isn't supported (as there will likely be special
caveats given this code is initially going to be written only for host machines
rather than for Fuchsia devices).

An example of how a handle might be acquired would be like the following
function, which can be used to connect to most any component on the
Fuchsia device by using the
[remote control FIDL definitions][rcs-fidl-definitions].

```c
extern zx_status_t ffx_connect_proxy(void* ctx, const char* endpoint_path,
                                     zx_handle_t* out);
```

This would return a FIDL channel to the protocol in question. The
`endpoint_path` should contain the necessary information to connect directly to
a specific FIDL protocol on the device via the component framework (at the time
of writing this is formatted as `"$moniker:expose:$protocol"`).

With support for socket/channel reading and writing, as well as event
signalling, it will be possible to replicate all FIDL functionality that FFX
and Overnet currently support, but in Python.

### The Python Language Bindings

The Python language bindings will define objects that use the established
create\*/close\*/destroy\* calls, written in C++ and offering a thin wrapper
around the FFX client library. It will also export an error type with use of
`ffx_error_str()`.

An example of this (slightly different) implementation can be found in the
Fuchsia Controller prototype [here][fuchsia-controller-prototype-py-bindings].

#### Higher Level Bindings

For actual usage of FIDL protocols, a higher level library will be available.
It will use `importlib` to hook into the FIDL intermediate representation (IR),
importing whatever IR is available either in a distributed `.pyz`, which would
be available in a developer's `PYTHONPATH`, or from a manually provided
directory. These hooks would create classes that interact with FIDL at runtime
analogous to what happens when C++ or Rust does compile-time bindings.

An example could look like:

```python
import fidl.fuchsia_developer_ffx
from fuchsia_controller_py import Context

def main(ctx: Context):
  h = ctx.connect_daemon_protocol(fidl.fuchsia_developer_ffx.Echo.Marker)
  proxy = fidl.fuchsia_developer_ffx.Echo.Client(h)
  print(proxy.echo_string(value = 'foobar'))

if __name__ == '__main__':
  ctx = Context(None)
  main(ctx)

```

Importing from `fidl.` would be caught by the library hook to search through the
existing FIDL IR namespace for something that matches `fuchsia.developer.ffx`,
generating classes based on the structs and protocols defined therein.

#### FIDL Wire Format Encoding/Decoding

For a first pass, the actual encoding and decoding would likely be done using
the `fidl_codec` libraries, then later this could be implemented totally in
Python.

`fidl_codec` includes code for reading from/writing to the FIDL wire format
based on FIDL IR definitions, as well as utilities like a
[value visitor][fidl-codec-visitor] that can be used to construct objects from
FIDL messages. It is used in the [fidlcat][fidlcat] tool. Encoding FIDL structs
to the wire format from IR-generated Python can be done using a visitor pattern,
creating a buffer that can then be written to the FIDL handle.

## Performance

### The FFX Client Library ABI

Most performance overhead will be encountered when crossing the FFX client ABI
boundary, namely Unix Domain Socket reading/writing and networking to/from the
Fuchsia Device.

Beyond this, the FFX client library will be copying FIDL reads
exactly twice, once to cross the Rust ABI boundary, and then one more time to
load the data from the C++ code into Python object(s). This can be addressed in
the future if it becomes an issue.

The average use case will involve lower-overhead usage of FIDL, namely calling
of simple functions with structs. At the time of writing more complex FIDL (like
VMOs) are not supported on the host, so should not be a source of concern for
performance.

## Ergonomics

### The FFX Client ABI

The FFX client binary is implemented in as small of a profile as possible to
prevent excessive boilerplate for implementing bindings in other languages.

### The Higher Level Python Bindings

The higher level Python bindings will use the FIDL IR to create class bindings
at runtime. This will make it possible to not only use Python code in a way
that behaves in a way that is congruent to other FIDL language bindings like
Rust, Dart, or C++, but will also make it possible to experiment with FIDL
protocols at runtime, making more iterative development possible.

If a FIDL service is defined outside of the tree, this will also make it
possible to access the service using the SDK (by first generating the FIDL IR,
then importing it into Fuchsia Controller).

## Security considerations

The main security implications in this RFC are related to FFX itself, which is
outside the purview of this document.

## Testing

### FIDL Wire Format

The FIDL team has a suite of tests (GIDL and DynSuite) against which the
encoding and decoding can be tested. This will work for either the case where
`fidl_codec` is being used (as it is not yet tested against these suites), or if
Python writes to/reads from the wire format explicitly.

### The FFX Client Library and the Python C bindings

These are lightweight enough that there isn't much to be done for unit testing
beyond verifying that certain exception criteria are met. In order to test the
FFX client library's `ffx_error_str()` a well-defined error can be used to
prevent relying on a specific string from within FFX itself.

Most of the testing of these pieces of code will be in integration and/or
end-to-end testing. A simple example that exercises every piece of code would be
using the `Echo` protocol for a non-Fuchsia-device dependent test. For a test
against the actual Fuchsia device, one could get an instance of the remote
control service proxy and call the `IdentifyHost` function.

## Documentation

Documentation for Fuchsia Controller will be kept on the `fuchsia.dev` website.

## Drawbacks, alternatives, and unknowns

### Drawbacks

#### First Class Python Support

While the FFX client library is intended to support bindings for other
languages, Python is not suited for larger productionized projects. Python,
arguably, allows for rapidly writing code and getting things working quickly at
the expense of ease of maintenance and readability.

### Alternatives

#### Why not Golang, Java, Rust, or Dart?

In our initial discussions with e2e testing teams, product facing developers,
and the connectivity team, the strong preference was for a scripting language
providing interactivity and ease of use. This pointed to Python for the first
language to have bindings.

### Unknowns

#### FIDL IR

Getting FIDL IR to the user out-of-tree with the SDK will be tricky to get
correct, as it has to be generated. So it may be necessary to add extra build
rules for Bazel in order to simplify dependencies.

#### FFX In the SDK

The above examples using the FFX daemon's protocols will not work out of tree,
as the FFX FIDL protocols have not yet been landed in the SDK. API review was
partially done but still needs to be completed. It is difficult to gauge how
long this might take as FFX still has a wide FIDL surface to cover, and is also
generally a moving target.

#### FIDL Codec

It may not be necessary at all to use the `fidl_codec` in the final result.
Using it will also involve adding more surface to the Python shared library,
which will then need to be phased out carefully.

Considered alternatives are:

* Generated code (which is how this is handled in C++ and Rust, for example).
* A `fidl_codec` analogue implemented in Python.

## Prior art and references

* [SL4F Evolution plan](https://docs.google.com/document/d/1OmbbBuUmNr8geG3sv15hR3Bb63qROqBoF2vTMNlwaeE/edit)
* [Host Driver Testing in Fuchsia](https://docs.google.com/document/d/1-1xYOxfWrpZsFfzN83_QQAhtMT6Pf5yCMH_luUtUius/edit)

[sl4f-overview]: /docs/development/testing/sl4f.md
[ffx-reference]: https://fuchsia.dev/reference/tools/sdk/ffx
[rcs-fidl-definitions]: https://fuchsia.googlesource.com/fuchsia/+/1bcf7bf350d0b37e0c50af3252785b417bb650d6/sdk/fidl/fuchsia.developer.remotecontrol/remote-control.fidl
[fuchsia-controller-prototype]: https://fuchsia-review.git.corp.google.com/c/fuchsia/+/761245
[fuchsia-controller-prototype-py-bindings]: https://fuchsia-review.git.corp.google.com/c/fuchsia/+/761245/6/src/developer/ffx/lib/fuchsia-controller/abi/fuchsia_controller_py.cc
[fidlcat]: /docs/development/monitoring/fidlcat
[fidl-codec-visitor]: https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/src/lib/fidl_codec/visitor.h
[fidl-service-definition]: /docs/glossary#service
[zx-channel-read]: /reference/syscalls/channel_read.md
