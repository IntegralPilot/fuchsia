# Fuchsia C library

This is the standard C library for Fuchsia.  It is in production use but still
under construction and constant renovation.

## Public header files

See [`include`](include) for full details on how the C library's public header
files are maintained.

## Fuchsia SDK

The public header files, ABI linking details, and prebuilt library binaries for
the Fuchsia C library are included in the [Fuchsia SDK](/docs/development/sdk).
Compilations by SDK users see them via the SDK's [sysroot](include/#sysroot).
Prebuilt shared library runtime binary files from the SDK are copied into each
Fuchsia package built, but later shared in both storage and memory at runtime
when different packages contain identical files.  None of the source code for
the C library appears in the SDK.  The C library implementation hermetically
uses many other internal APIs that are never made available to SDK users.

## llvm-libc

The Fuchsia C library is organized around the code of the [llvm-libc] project.
[`//third_party/llvm-libc/src`](/third_party/llvm-libc/src) tracks the
[`libc`][llvm-libc-github] subdirectory of the main LLVM repository.

The Fuchsia C library takes the llvm-libc source tree as just that: a library
of source files to draw from.  The LLVM CMake build supported by the llvm-libc
project is never used to build the Fuchsia C library; the LLVM repository on
its own does not provide all the code for a full C library usable on Fuchsia.
Instead, the [GN templates](libc.gni) and [`BUILD.gn`](BUILD.gn) files in this
directory and its subdirectories drive the entire build.

## Source code layout

The structure of this source directory largely mirrors LLVM's [libc/src]:
there is approximately one subdirectory corresponding to each public header
file in the standard C API (and some subset of POSIX and common extensions).
A few individual implementation source files are in the top-level directory
and a few subdirectories are for parts of the implementation that do not
correspond to public header file names.

Most subdirectories contain build rules for much more code than is there.
Often most or all of the implementation code is coming from the corresponding
subdirectory mirrored from LLVM's [libc/src].  When the implementation code is
here, test code may be coming mostly or entirely from LLVM's [libc/test/src].

Code that corresponds fairly closely to its llvm-libc counterparts uses a
`fuchsia` subdirectory for Fuchsia-specific code, though there aren't usually
`linux` subdirectories here where LLVM's [libc/src] subdirectories have them.
There may also be architecture-specific subdirectories of the topical
subdirectories here, either directly or under `fuchsia`; for example,
[`setjmp/fuchsia/aarch64`](setjmp/fuchsia/aarch64) contains code that is
specific to the Fuchsia Compiler ABI on AArch64.

Some larger test code is in [`test`](test) and its subdirectories.  Unit test
code for implementation pieces in subdirectories sits alongside that code in
source files called `*-tests.cc` and the like.  This differs from LLVM's
layout using [libc/test/src] in parallel to [libc/src].

## scudo, gwp_asan

The [Scudo] allocator is the main memory allocator built into the Fuchsia C
library to provide the standard C `malloc` and related APIs, plus extensions.
[GWP-ASan] is an extension of [Scudo] that enables runtime-configurable,
probabilistic use of an alternate allocator meant to turn buffer overrun and
use-after-free bugs into immediate "safe" crashes with post-mortem telemetry
support, at tunable marginal cost overhead suitable for production use.

The Fuchsia C library tracks this code from the LLVM repository very much as
it does the [llvm-libc] code.  In particular,
[`//third_party/scudo/src`](/third_party/scudo/src) and
[`//third_party/gwp_asan/src`](/third_party/gwp_asan/src) track LLVM's
[`scudo`][scudo-github] and [`gwp_asan`][gwp_asan-github] respectively.  All
build rules for both are entirely in the [`scudo` subdirectory here](scudo).

## musl

The Fuchsia C library started as a fork of [musl], an alternative C library
meant only for Linux.  This was a hard fork not formally tracked against any
upstream [musl] source repository.  The heavily modified copy is found at
[`//zircon/third_party/ulib/musl`](/zircon/third_party/ulib/musl).  All
essential code has been heavily modified over the years, and later [musl]
development is mostly not reflected in the code here.  As new replacement code
is added in the subdirectories here or adopted from [llvm-libc], old code is
removed from that source directory.

All this code is expected to be removed eventually.  What is not replaced with
new code, whether specific to Fuchsia or shared with [llvm-libc], will either
be dropped entirley as unnecessary and unused by Fuchsia's users, or will be
migrated into compatibility layers that outside of the core C library.

## Auto-rollers

[llvm.googlesource.com] provides mirrors of the official LLVM [monorepo] as
well as separate repositories for subtrees of it.  These are almost always up
to date within a few minutes of new changes landing upstream on GitHub.

The [llvm-libc-github], [scudo-github], and [gwp_asan-github] subtrees are
used via Fuchsia's `//third_party` as described above, each pinned in the
[`jiri` manifest](/manifests/third_party/all) to the latest revision.

Each of these has an auto-roller that runs in Fuchsia's [LUCI] infrastructure
both periodically, and soon after any time there is a new commit in a tracked
mirror repository.  That job uploads a [Gerrit] change of the one-line edit to
change the Git revision in the manifest file, as if by `jiri edit`.

These jobs make sure to run all the testing bots that Fuchsia CI would run
(not only those normally run by CQ).  If all goes well, the change gets an
automatic approval and lands without manual intervention.  If not, the job
marks the change "abandoned" (but another attempt may reopen it later instead
of starting a freshing one that's identical).

[Maintainers](OWNERS) get [Gerrit] notifications about the roll attempts.
[File bugs] and/or consult them with any issues observed.

### Roll flake

When an auto-roller job fails, the next new revision will trigger a fresh
attempt with the new revision.  Only one attempt for each auto-roller is left
building or running tests at the same time.  So the next fresh attempt may be
immediately after the last one finishes (whether in success or failure), but
will always attempt only the latest upstream revision available when it's
ready to try again (not just one commit at a time).

If there are no triggered runs after some hours, then the auto-roller will run
another job anyway.  This catches any missed triggers due to GitHub or
infrastructure hiccups.  More often, it catches cases where a previous roll
attempt failed for reasons having nothing to do with the roll itself.  The
Fuchsia CI is temporarily broken somehow, the tree is closed for too long,
some of the many, many bots flake in CQ though they didn't have CI flakes
cited by build gardeners, etc.  Most often, another attempt after unrelated
fires of the day have been put out, or things go eerily quiet in the night,
will succeed without intervention.

### Coordinated upstream changes

The LLVM [monorepo] makes it possible to simultaneously change [llvm-libc],
[Scudo], and [GWP-ASan] in a single commit.  However, that upstream commit
will be mirrored as separate commits in each slice repository as used here.
Since each one is pinned separately and has its own auto-roller that updates
only one pin at a time, things don't work automatically for the Fuchsia build
that do work automatically when tracking the full LLVM repository as a unit.

In these, cases [_manual rolls_](#manual-rolls) are the solution.

### Upstream regressions

On occasion an upstream LLVM change will land and not be seen as a problem
upstream, but break the Fuchsia build.  This happens for a number of reasons:

 * Insufficient upstream presubmit test coverage and upstream CI slower than
   Fuchsia's infrastructure so the auto-roller is the first user to notice.
 * Insufficient test coverage upstream altogether.  There aren't so many bots.
 * Different compiler versions on upstream bots than the builds pinned in
   Fuchsia's [//manifests/toolchain](/manifests/toolchain).
 * Looser warning settings in LLVM Cmake build than in the Fuchsia build.
 * Upstream changes interact with Fuchsia-specific code not visible upstream.

When there's a lack of testing upstream, it's best to try to address that
upstream directly.  Similarly, the [llvm-libc] project is largely amenable to
amping up the warning settings in the CMake build to be as stringent as the
Fuchsia build's, and making that change in an LLVM checkout's build files
should enable a CMake build to reproduce the source code regressions that need
to be fixed to compile warning-clean.  If upstream changes simply land that
then enable the auto-roller to succeed with a newer LLVM revision, then that's
usually the ideal outcome (it can happen while Fuchsia maintainers sleep!).

On occasion policy or code style issues will differ between LLVM and Fuchsia
such that warning settings should be changed in [`BUILD.gn`](BUILD.gn) when
compiling the shared code.  For other kinds of code interactions it may
similarly be possible to land changes in Fuchsia that are compatible both with
the current pinned upstream versions so they can land independently, and newly
recover that compatibility with the latest LLVM code such that the auto-roller
will become able to succeed with the same LLVM revision that failed before.
That is often the (second) easiest way to get back to smooth operation.

In more complicated situations, or just for quicker results for an impatient
maintainer, [_manual rolls_](#manual-rolls) may be required.

### Header updates

Some header files come in various ways from [llvm-libc] and the auto-roller
implications of this are explained in [their own entire directory](include).
At the end of all those complications specific to header files, really it's
just that [_manual rolls_](#manual-rolls) are the solution.

### Manual rolls

The [manifests](/manifests) controlling the pinned LLVM revisions are part of
the Fuchsia source tree directly in the [fuchsia.git] repository.  This means
that a "roll" is nothing more than a normal [fuchsia.git] commit, always
tested, reviewed (except when by auto-rollers), and landed via [Gerrit].

A maintainer can do those commits as well as the auto-roller.  This can be done
by hand edits in [the XML file](/manifests/third_party/all) or via the
command-line `jiri edit` tool.  It has to be invoked with some idiosyncratic
details from the manifest.  The [`manual-roll.sh` script](manual-roll.sh) helps
with this.

There's rarely any use for a manual roll that moves one of the pins to an
older revision than it was before, which the auto-roller never does.  It can
land and might not hurt.  But the auto-roller will just come along and try to
update it back to the latest available upstream revision sooner or later.

Usually a `git remote update` and `git checkout origin/main` in the local
`//third-party/llvm-libc/src` and its brethren makes local development match
the best case update that the auto-rollers are attempting.  (This is what the
[`manual-roll.sh` script](manual-roll.sh) without arguments does.)  Each can be
separately adjusted to a different revision as needed such that they all work
together in the Fuchsia build.  (The same script can then be used with `HEAD`
as the argument to do the `jiri edit` command to match your local checkout
state for each upstream mirror repository.)

In the same way, any changes needed to the source code or build files here to
work with the new revisions can be included in the same change and land as an
atomic commit when necessary.  If need be, even unrelated `//third_party`
pinned revisions or prebuilt package pins can be changed in lock-step in the
same way as the rest of the Fuchsia code outside libc per se.  For example,
[header file changes that need golden-file updates and API review](include)
might also need coordinated changes to out-of-spec users of libc API headers;
other libc changes might need to be made in lock-step with a compiler upgrade.
While many changes can be made atomically, landing separate incremental changes
and minimizing other changes landed in the same commit with a roll is prudent.

It's a good idea to watch for the next run of each relevant auto-roller after
landing a change that touches [manifests](/manifests).  When manually-updated
pins match their mirror's newest revision it will succeed without uploading
anything.  The [LUCI] UI can show all jobs (may require login) and allows
those in the maintainer ACLs to manually trigger a job without waiting for the
periodic schedule or a mirror repository change.  Whenever there's a previous
failed [Gerrit] change, a comment posted by the auto-roller includes the
specific URL that makes it easy to find the right part of the UI to see the
job history and schedule; or to trigger a new job.

## Dynamic Linking

The [`ld`](ld) subdirectory does not correspond to a public API per se, but
instead contains code that works with the dynamic linking regimes provided by
[`//sdk/lib/ld`](/sdk/lib/ld).  **TODO(https://fxbug.dev/342469121)**

---

[File bugs]: https://issues.fuchsia.dev/issues/new?component=1478304
[GWP-ASan]: https://llvm.org/docs/GwpAsan.html
[Gerrit]: https://fuchsia-review.googlesource.com/
[LUCI]: https://luci-milo.appspot.com/ui/p/turquoise/g/global_roller/builders
[Scudo]: https://llvm.org/docs/ScudoHardenedAllocator.html
[fuchsia.git]: https://fuchsia.googlesource.com/fuchsia/+/HEAD
[gwp_asan-github]: https://github.com/llvm/llvm-project/tree/main/compiler-rt/lib/gwp_asan
[libc/src]: https://github.com/llvm/llvm-project/tree/main/libc/src
[libc/test/src]: https://github.com/llvm/llvm-project/tree/main/libc/test/src
[llvm-libc-github]: https://github.com/llvm/llvm-project/tree/main/libc
[llvm-libc]: https://libc.llvm.org/
[llvm.googlesource.com]: https://llvm.googlesource.com/
[monorepo]: https://github.com/llvm/llvm-project
[musl]: https://musl.libc.org/
[scudo-github]: https://github.com/llvm/llvm-project/tree/main/compiler-rt/lib/scudo/standalone
