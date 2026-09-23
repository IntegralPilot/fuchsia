<!--
    (C) Copyright 2023 The Fuchsia Authors. All rights reserved.
    Use of this source code is governed by a BSD-style license that can be
    found in the LICENSE file.
-->

# Testing Third Party Changes

## Preface

This doc is mostly focused on testing changes to Scudo, GWP-ASan and LLVM-libc,
as such the doc uses these as examples but the concepts will transfer to any
third_party/ project. It is intended both for Fuchsia developers and for
developers of upstream projects who may not have a Fuchsia checkout or be
familiar with the build.

### Getting the checkout

For those who don't already have a Fuchsia checkout, you can follow the steps
[here](/docs/get-started/get_fuchsia_source.md).

## Running the tests

Further documentation on building and running tests can be found in
[Configure and build Fuchsia](/docs/get-started/build_fuchsia.md) and
[Run Fuchsia Tests](/docs/development/testing/run_fuchsia_tests.md) respectively.
For a quick summary see below.

All methods will refer back to how the tests are actually run. All third_party/
tests will be run differently, if they are run at all. A good place to start
is to grep around `*.gn{,i}` files to see where in the build these projects are
being referred to. In the case of Scudo, it is part of libc's build and most
of the build logic concerning it and its tests can be found in
[//sdk/lib/c/scudo/BUILD.gn](/sdk/lib/c/scudo/BUILD.gn).
For this particular target it is depended on by `//sdk/lib/c:tests`.

First run `fx set core.x64 --with //sdk/lib/c:tests`, see the
[build](/docs/get-started/build_fuchsia.md) docs on more details, but this
configuration is a good default. This only needs to be run once.

Then run `ffx emu start --headless`. This starts the emulator and will build
anything it needs. Also run `fx serve` either in the background or foreground
but create a new terminal session in the case of latter. Both of these need
only be run once, but need to be run again if they die or your machine is
restarted. Note: if you `jiri update` it is good practice to rebuild and
restsart the emulator and package serve.

To run the tests you can look to see which test target ends up depending on
the test you are interested in. Scudo tests are rolled into libc tests which
are `libc-unittests-pkg`, some GWP-ASan tests are there as well as
`gwp-asan-test-pkg`. Run `fx test -v libc-unittests-pkg gwp-asan-test-pkg` to run just those tests.
When in doubt, `fx test -v` can be run without arguments to run all tests, which
if you only included `//sdk/lib/c:tests` won't take too long.

## Testing a change locally

To test a change locally, simply apply the patch to the respective //third_party
directory. Usually this will be //third_party/${proj}/src, though in the case
of GWP-ASan it is in //third_party/scudo/gwp-asan. After the diff has been
applied run the tests as normal and targets will rebuild against your new
changes. This is a great way to do rapid iteration on a change you are
submitting upstream or testing someone elses change before they submit.

Tip: For testing out LLVM reviews, you can use
`curl -L 'https://reviews.llvm.org/DXXXXXX?download=1' | git apply -p${num}`
to easily apply a patch. DXXXXXX should be replaced with the patch you are
testing. ${num} should be replaced given the difference in directory hierarchy
between where the project lives in llvm and in third_party. In the case of
Scudo, use `-p5`.

Tip: If you are submitting a change to an upstream repo and have that change
locally

## Testing a failed roll

All of scudo, gwp-asan and llvm-libc, are rolled in by auto-rollers which will
fail if they break the build or if tests stop passing, either theirs or other
in tree tests. To test this locally, apply the diff created by the auto-roller
to `//integration`, this will be just a one line change bumping up the revision
on that project. To modify the commit hash manually to test at another
revision, either change the file by hand or run
`jiri edit -project=${proj}=${hash} ${manifest}`, where manifest will likely be
//integration/fuchsia/third_party/flower. Then run `jiri update -local-manifest`
this will pull in the new changes specified in your `//integration` directory.
Here, `jiri` will likely give an error that there are uncommitted changes in
`//integration`, this won't actually stop `jiri` from correctly updating the
third party project. If this bothers you, create a new branch in `//integration`
and commit your changes, this will make `jiri` simply warn that `//integration`
is not on `JIRI_HEAD`. To ensure your project was correctly updated, run
`cat ${path_to_proj}/src/.git/HEAD`. Run tests as normal.

## \[Googlers Only\] Leveraging bots

Only Google employees can make push changes to `//integration` and view tqr/

Some failures will only happen on certain devices and without having access to
them locally, it is impossible to reproduce locally. These cases should be
exceedingly rare, and as such the workflow is not particularly well refined.
First, find the project you want to test, in the case of Scudo it looks like:
```
<project name="scudo"
         path="third_party/scudo/src"
         remote="https://llvm.googlesource.com/scudo"
         revision="b0c7c7b80b6dfa2cd0b5dae98cb1ea33d31d2497"/>
```
Make a git clone of the remote, here that is
`https://llvm.googlesource.com/scudo`, and push to an accessible git
host. See go/gob/users/user-repository, go/gob/users/team-repository and go/gob/users/new-host. Change the project's remote to point to your
host repos address. Make changes to your clone and change the revision to the
revision you want to test at.
After that, push your change, go to the review and add the specific bots you wish to test.

Example: tqr/634497
