<!-- Generated with `fx rfc` -->
<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0256" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

<!-- This should begin with an H2 element (for example, ## Summary).-->

## Summary

Propose to bundle Lacewing tests as compiled, Rust host-binaries for Fuchsia's
near term test-specific Python application bundling and distribution
needs (e.g. Driver conformance, CTF).

This proposal does not foreclose [alternatives](#alternatives) to be explored in
the future.

## Motivation

With the recent addition of the RTC driver conformance test in the Fuchsia IDK,
we now have a working and load-bearing mechanism for delivering versioned
in-tree Python [Lacewing][Lacewing] E2E tests for out-of-tree (OOT) execution.

The ability to distribute Lacewing tests to SDK consumers directly supports
[Fuchsia's 2024 roadmap](/docs/contribute/roadmap/2024)
(i.e. Driver conformance tests). Additionally, other testing efforts that
require versioned test packaging and distribution such as
[Platform Expectation Tests][PETS] and Host Tool [Compatibility Tests][CTF] will
benefit as well.

Before we scale up adoption of this test distribution model, we must ensure it
is sustainable and get ahead of potential technical debt. Specifically, the
current requirement for downstream Lacewing test consumers to supply their own
Python runtime for test execution is error-prone - this proposal addresses this
shortcoming by removing the requirement altogether.

## Stakeholders

_Facilitator:_

- abarth@google.com

_Reviewers:_

- abarth@google.com
- chaselatta@google.com
- hjfreyer@google.com
- keir@google.com
- tmandry@google.com
- tonymd@google.com

_Consulted:_

- awdavies@google.com
- crjohns@google.com
- cpu@google.com
- jamesr@google.com


_Socialization:_

This proposal was socialized via a Google Doc with members from the FEC,
Pigweed, Toolchain, SDK Experiences, and Tools team. The proposal was also
discussed in a FEC meeting with support for formal RFC ratification.

## Goals

*   Eliminate Python runtime compatibility issues when Lacewing test binaries
are run OOT
*   Package Lacewing Python tests as single hermetic executables
    *   Support C extension libraries (e.g.
[Fuchsia Controller][Fuchsia Controller])
    *   Support data dependencies (e.g. FIDL IR files - currently required by
    Fuchsia Controller)
*   Support bundling Lacewing tests where they are defined
*   Support Linux host environments

## Non Goals

*   Support Windows host environments
*   Support Mac host environments

## Proposal

We propose to use a custom Rust-based approach for Fuchsia's immediate Python
application bundling needs as it's the approach that is both
[feasible][Soak testing] and aligns with all of the [goals above](#goals).

### Overview

All of the Python-components required to run a Lacewing test will be embedded in
a compiled Rust binary as its data-resources (inlined in the binary via Rust's
`include_bytes!` macro). When run, the Rust binary extracts its Python contents
into a temporary directory, constructs a command, and executes the Python
application.

Compared to the leading alternative, PyInstaller, the custom Rust-based approach
is ultimately preferred at this time due to its flexibility to be used in
Fuchsia.git (more info in PyInstaller's
[rejection rationale section](#pyinstaller-rejection-rationale) below). The
following summarizes the main tradeoffs between the approaches:

* Advantages of custom Rust-based approach:
   * Can be used in Fuchsia.git
   * Is already [implemented](https://fxrev.dev/c/fuchsia/+/1061772) and
     successfully runs in both local and infra environments
   * Adopts hermeticity guarantees from Fuchsia's existing Rust host toolchain
   * Does not require the additional maintenance of a dynamically linked Python
     runtime
   * Does not require updating Fuchsia Controller to locate embedded FIDL-IR
     files

* Advantages of PyInstaller:
   * Built-in generic support for all Python applications
   * Better line-of-sight for supporting non-Linux host platforms (e.g. Windows)

## Implementation

A Lacewing Rust binary comprises of 3 logical sections: Lacewing artifacts
embedding, artifacts extraction, and test execution.

### Embedding

All of the components required for running a Lacewing tests will be embedded
into the Rust binary as a single archive.

#### Contents

The embedded archive contains the following:

* Statically-linked Python Runtime and its standard libraries (Python modules)
* Lacewing test in PYZ format (consistent with `zipapp` bundling used in
  Fuchsia.git)
* Fuchsia controller shared libraries
* FIDL IR files (currently required by Fuchsia Controller)

#### Compression

For simplicity, all of the contents listed above are compressed into a single
archive. The current implementation uses `ZIP` format which produces stripped
linux-x64 host binaries of approximately 40MB. Future iterations may opt for
compression formats that have better compression ratios like `zstd`
(compression performance is not as significant in the context of host-driven
system tests).

#### Build

To bundle arbitrary Lacewing tests, we will add GN build automation to
dynamically supply Lacewing artifacts for embedding.

We achieve this through the combination of the
[rustc_embed_files()](https://cs.opensource.google/fuchsia/fuchsia/+/main:build/rust/rustc_embed_files.gni;l=11;drc=c9f679bc6997f95834b5b285f820a99494f18e1c)
and
[rustc_binary()](https://cs.opensource.google/fuchsia/fuchsia/+/main:build/rust/rustc_binary.gni;l=151;drc=6b52f09df181721d2d9a9489280d0a72b76945d8)
GN templates where the former provides the *test-specific* data resources as a
Rust library while the latter contains the *test-agnostic* `main()` logic for
[extraction and execution](#extraction-and-execution).

### Extraction and execution

When run, the Rust binary first unpacks its embedded resources in a temporary
directory. Then it will build the command for running the Lacewing test by
referencing the extracted content. Finally, the command is executed and the
Rust binary exits. To ensure hermiticity, we will also set the `PYTHONPATH`
environment variable in the command to ensure that ambient Python installations
and libraries will not be used.

## Performance

*Build time* - Lacewing test bundling will only run on a small subset of
Fuchsia's portfolio of Lacewing tests so the build overhead of Rust binary
bundling is negligible.

*Run time* - Due to the E2E testing nature of the Python applications being
bundled, they are not sensitive to the minor start-up overhead that occurs
during the extraction phase. Empirical infra data shows <6s for unoptimized
builds which is negligible given these tests average around 1 minute to run.

*Size* - The output executable size is around 40MB which is [similar](#size) to
alternatives like PyInstaller. However, there is still room for improvement in
the choice of compression algorithm used during [embedding](#embedding).

Below are opportunities to optimize the performance each of the dimensions
above, these can be explored as the number of bundled Lacewing tests increases:

* Build time: Build a "stem" Rust ELF binary that contains the
test-agnostic artifacts (e.g. runtime, stdlibs, C extensions) and
concatenate it with test-specific Zip archives (e.g. test.pyz). The "stem" ELF
can then self-extract via `ZipArchive` to access its Zip content just like the
current proposed approach; except the Rust compilation process is only performed
once instead of for every bundled Lacewing test.

* Run time - Use GN config to optimize Rust compilation for speed. The
time-to-extract can be measured and exported to a performance tracking backend
to prevent regressions.

* Size: Similar to the build time optimization above, by splitting the Rust ELF
binary into test-agnostic "stem" and test-specific Zip archive, we can
distribute the large "stem" separate from the significantly smaller
test-specific Zip archives. This allows us to save storage and network bandwidth
for redundantly including the "stem" in each bundled Lacewing test. More info in
[Executable size](#executable_size).

## Ergonomics

This proposal improves the UX of OOT Lacewing test execution - integrators now
only needs to launch a single test executable.

## Backwards Compatibility

The addition of hermetic mode (Rust bundling) will not require any changes to
the Lacewing test sources. In other words, Lacewing tests can be run in
both hermetic (`./test`) and non-hermetic (`./python test.pyz`) modes without
any changes to the Python source code.

## Testing

Both the stabilities of 1) the Rust bundling procedure and 2) the resulting
hermetic tests will be [soak-tested][Soak testing] in Fuchsia infra. We will
benchmark the test stability against its non-hermetic counterpart; if no obvious
discrepancies in flake/fail rate are observed, we will consider the Rust-based
bundling approach to be ready for production use (e.g. CQ and SDK distribution).

## Risks & Resources

### Executable size

Assuming a storage footprint of ~40MB per bundled Lacewing binary, this could
cause concerns as we scale up the number of distributed tests to reasonable
estimates (e.g. 4GB for 100 tests). To mitigate, the following can be explored
in parallel to scale storage and bandwidth costs sublinearly with the number of
Lacewing tests bundled.

1.  Distribute all tests in a monolithic Rust application.
    * The largest components of a Lacewing archive (Python runtime, standard
    libs, and Fuchsia Controller extensions) are distributed once.
    * The size savings this approach provides increases with the number of
    Python applications we distribute OOT.

1.  Distribute tests independently.
    * Similar to above, the largest components of a Lacewing archive are
    distributed once in a single Rust application.
    * The test PYZs (500kB) are distributed separately and provided to the main
    test application as a runtime argument.

1.  Reduce the size of Fuchsia Controller.
    *   This will decrease organically due to Fuchsia Controller's migration
    towards static Python binding (upstream run-time logic to `fidlgen`)
        *   `fidl_codec.so` will be removed
        *   Static Python FIDL bindings will be less verbose and thus smaller
        than FIDL IR files

1.  Explore other compression formats and parameters as mentioned
[above](#Compression).

### Cross-platform feasibility

Coupling our Python bundling solution to Fuchsia's Rust host toolchain means
we will get Linux x64 support for free but we will also be limited to the
the current set of supported platforms. At the moment, Linux x64 is sufficient
for our immediate needs. However, in the case where additional host platform
support (e.g. Windows) becomes a requirement, we will need to collaborate with
the Fuchsia Rust and Fuchsia Build teams for feasibilty scoping.

At the time of writing, there are no known non-Linux-x64 host platform
requirements for OOT Lacewing tests.

### Limited support

The current proposal is only scoped to support Lacewing Python applications.
However, if the need arises, this approach can be reasonably extended to support
generic Python applications with much less effort than exploring the
alternatives (e.g. PyInstaller/PyOxidizer).

The [alternatives](#alternatives) are worth considering if there are large
requirement changes such as:

* Windows support
* Bit-for-bit reproducibility
* Do not launch Python under-the-hood

## Security considerations

Shipping Rust host-binaries in Fuchsia's SDK is a well established process (e.g.
`ffx`) so there are no additional security risks being introduced by applying
the same approach for Lacewing tests.

## Privacy considerations

N/A

## Documentation

N/A

## Alternatives

### PyInstaller

This large section on PyInstaller is intentionally included for posterity. The
findings below help inform our proposal today to opt for a custom Rust-based
approach, and will help provide context for any future efforts.

#### PyInstaller rejection rationale

Due to PyInstaller's
[licensing](https://pyinstaller.org/en/stable/license.html), it can only be used
in Fuchsia.git as a prebuilt artifact under the "Development targets" section of
[Fuchsia's open source licensing policies][OSRB policies]. However, PyInstaller
is designed to run as source and does not easily package into a prebuilt. At
this time, we are not motivated to solve this non-trivial problem given we
have a Rust-based solution that is working, available in-tree, and serves our
immediates needs.

#### Overview

[PyInstaller](https://pyinstaller.org/en/stable/license.html) is an open source
3rd party Python library used for bundling Python applications and their
dependencies into self-contained executables that can run on end user systems
that do not have Python installed. UIt is one of the more popular solutions in
the space of Python application packaging due to its ease of use and maturity
(active releases
[since 2015](https://github.com/pyinstaller/pyinstaller/releases/tag/v1.0)).

When PyInstaller is run in "one-file" mode, it bundles the runtime it's launched
with into the output executable along with all of the application modules. When
the output executable is run, PyInstaller's bootloader extracts its embedded
archive (containing the Python interpreter, built-in/user-supplied Python
modules, built-in/user-supplied shared libraries, and data files) into a
temporary directory, then executes the interpreter in a hermetic context.

With PyInstaller, we are able to bundle a Lacewing test into a single hermetic
portable executable (Python runtime + test modules + library modules + Fuchsia
Controller C extensions + data dependencies such as FIDL IR) to run on across
diverse Linux host environments.

Unfortunately, statically linked Python runtimes like Fuchsia's vendored Python
are not
[compatible with PyInstaller](https://www.pyinstaller.org/en/stable/when-things-go-wrong.html)
(related issues: [1](https://github.com/pyinstaller/pyinstaller/issues/4071),
[2](https://github.com/pyinstaller/pyinstaller/issues/8111)). The solution is to
build a dynamically linked version of the same CPython runtime in Chromium's
[3PP](https://chromium.googlesource.com/chromium/src/+/HEAD/docs/cipd_and_3pp.md)
Infra (Third Party Packages). Once built, the dynamically linked version
(**_DL-CPython3_** henceforth) can be pinned as a prebuilt in Fuchsia.git to
serve Fuchsia's Python application packaging needs.

Since the source code to build _DL-CPython3_ is the same as the statically
linked version (**_ST-CPython3_** henceforth) which is already used in Fuchsia,
no additional licensing approval is required for _DL-CPython3_.

#### Working prototype

This [Fuchsia.git CL](https://fxrev.dev/c/fuchsia/+/1013301) demonstrates the
feasibility of packaging a Lacewing test via PyInstaller through Fuchsia's GN
build system. With the prototype CL, we confirmed that it is possible to produce
hermetic Lacewing Python test executables via running PyInstaller at source at
GN time.

A demonstration of the CL working in Fuchsia infra is not feasible at the time
of writing due to the [unsubmitted dependencies](#unsubmitted-dependencies) used
in the prototype. Though we don't anticipate any challenges with infra-support
once those dependencies are made available in Fuchsia source code.

##### Unsubmitted dependencies

* DL-CPython3: This is now built in
[Chromium 3PP](https://chrome-infra-packages.appspot.com/p/infra/3pp/tools/cpython3_dyn/linux-amd64)
but has not been pinned in Fuchsia.git yet.

* PyInstaller libraries: PyInstaller and its transitive Python dependencies have
completed OSRB review but can only be used in Fuchsia.git as a prebuilt binary.

#### Fuchsia Controller

Fuchsia Controller would need to be updated to locate its data dependencies at
run-time. When a PyInstaller executable is run, its contents are unpacked and
executed from a temporary directory. With this
[patch](https://fxrev.dev/c/fuchsia/+/1013301/7/src/developer/ffx/lib/fuchsia-controller/python/fidl/_library.py)
Fuchsia Controller becomes PyInstaller-aware and looks for FIDL IRs in the
temporary directory when run as a PyInstaller executable.

##### Hermeticity

PyInstaller's output was confirmed to be hermetic from inspecting its build
metadata - [Analysis.toc][Analysis TOC]. All of the content bundled within are
sourced from `$FUCHSIA_DIR` - in other words, no ambient system shared libraries
are included.

Additionally, running `ldd` on the PyInstaller output shows a set of shared
libraries dependencies that are ubiquitous and reliably compatible on modern
Linux distributions.

```sh
~/fuchsia$ ldd dist/soft_reboot_test_fc
linux-vdso.so.1
libdl.so.2 => /lib/x86_64-linux-gnu/libdl.so.2
libz.so.1 => /lib/x86_64-linux-gnu/libz.so.1
libpthread.so.0 => /lib/x86_64-linux-gnu/libpthread.so.0
libc.so.6 => /lib/x86_64-linux-gnu/libc.so.6
/lib64/ld-linux-x86-64.so.2
```

To prevent hermeticity from eroding over time, a build-time check can be added
to confirm the `ldd` output of a PyInstaller executable exists in an allowlist.

##### Size

Without any considerations for size optimizations in the prototype, the
soft-reboot Lacewing test packaged via PyInstaller comes out to around 43 MB
(for reference, the PYZ version is ~400KB).

There are various unexplored opportunities to reduce the size of a PyInstaller
Lacewing test:

1.  Use [UPX](https://pyinstaller.org/en/v3.3/usage.html#using-upx)
compression when running PyInstaller.
1.  Exclude unused built-in extension modules.

#### Runtime management and maintenance

Fuchsia team will need to maintain both _DL-CPython3_ (for versioned Lacewing
test packaging) and _ST-CPython3_ (for everything else) and ensure the two
versions are functionally equivalent and are updated in lockstep (e.g. share the
same CIPD version tag). This would require collaboration with Google's internal
PEEP Software Deployment team to streamline the 3PP configurations of how the 2
versions are built to programmatically prevent divergence.

#### Cross-platform feasibility

[PyInstaller does not support cross-compilation](https://pyinstaller.org/en/v4.1/usage.html#supporting-multiple-operating-systems)
so we would need to run PyInstaller in respective target environments
(e.g. Windows, Mac) to generate host-platform-specific executables.

As an example, to support Windows end users, we'd need to run PyInstaller on a
Windows machine using a Windows version of _DL-CPython3_. At time of writing,
Fuchsia has no such Windows-based build environments so it would require scoping
to add support in our build fleet. As for _DL-CPython3_ for Windows, Chromium
3PP already supports building ST-CPython3 for multiple platforms (including
Windows) so we should be able to trivially expand support for platforms beyond
Linux if needed.

### PyOxidizer

[PyOxidizer](https://pyoxidizer.readthedocs.io/en/stable/) is another attractive
generic Python application packaging option and has various advantages over
PyInstaller:

*   Hermeticity - PyOxidizer's
output is a compiled binary whereas PyInstaller's output is an archive that
needs to be unpacked into Python bits (e.g. built-in Python extensions) at
run-time.
*   Speed - PyOxidizer's output
does not need to be unpacked to run which makes its startup time faster than
PyInstaller's output.
*   Security - PyOxidizer is
written in Rust, which is a language with better security guarantees than
PyInstaller's Python and C.


With that said, these advantages are mostly negligible in the E2E testing
context.

#### PyOxidizer rejection rationale

Ultimately, PyOxidizer is not chosen because it does not satisfy the
[goal](#goals) of bundling C extension libraries in a single executable. Cursory
explorations with PyOxidizer failed to bundle a toy C extension within its
output executable. While it's possible that the experiments had some
misconfiguration given PyOxidizer's high degree of configurability, resources
on such topics are sparse due to PyOxidzer's relative novelty and lower adoption
so we've shelved further explorations at this time.

## Prior art and references

* [Fuchsia Controller][Fuchsia Controller]
* [What Tests To Write][What Tests To Write]


<!-- xrefs -->
[Analysis TOC]: https://pyinstaller.org/en/stable/advanced-topics.html#table-of-contents-toc-lists
[CTF]: /docs/reference/testing/what-tests-to-write.md#compatibility-tests
[Fuchsia Controller]: /docs/contribute/governance/rfcs/0222_fuchsia_controller.md
[Lacewing]: /docs/reference/testing/what-tests-to-write.md#host-driven_approach_to_spec_conformance_tests
[OSRB policies]: https://g3doc.corp.google.com/company/thirdparty/fuchsia.md
[PETS]: /docs/reference/testing/what-tests-to-write.md#platform-expectation-tests
[Soak testing]: https://ci.chromium.org/ui/test/fuchsia/host_x64%2Fobj%2Fsrc%2Ftesting%2Fend_to_end%2Fexamples%2Ftest_soft_reboot%2Fsoft_reboot_test_fc.hermetic.sh
[What Tests To Write]: /docs/reference/testing/what-tests-to-write.md
