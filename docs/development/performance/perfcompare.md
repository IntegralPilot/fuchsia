# Perfcompare: Performance comparison try builder

The "perfcompare" try builder is an optional CQ try builder for
measuring the performance impact of a change without landing it
(i.e. for pre-submit performance testing). It runs performance tests
both with and without a CL applied and compares their results to see
if there were any performance regressions or improvements.

Googlers can refer to the [Google-internal perfcompare
docs][internal-doc] for some additional documentation.

## How to use it

### For fuchsia.git CLs

To run perfcompare on a Gerrit CL, do the following:

*   **Start a build:** Select "Choose tryjobs" in the Gerrit Web UI,
    and select one or more of the perfcompare builders from the list
    of builders. A quick way to do that is to type "perfcompare" into
    the search field, which will filter the list to display the
    available perfcompare builders.
*   **Get the results:** A link to the try builder's results page will
    appear on the CL in Gerrit. When the builder run is finished, the
    results will be under **"compare perf test results without and
    with CL" -> "stdout" (or "raw")** on the build page.

These perfcompare builders are currently available and supported for
running `fuchsia.git`'s performance tests:

*   `terminal.x64-release-perfcompare`
    ([recent builds](https://ci.chromium.org/p/fuchsia/builders/try/terminal.x64-release-perfcompare)):
    This runs `fuchsia.git`'s performance tests on Intel NUCs
    (x64). This is the perfcompare version of the
    `terminal.x64-release` builder (i.e. it runs the same set of
    performance tests as that builder).

*   `terminal.vim3-release-perfcompare`
    ([recent builds](https://ci.chromium.org/p/fuchsia/builders/try/terminal.vim3-release-perfcompare)):
    This runs `fuchsia.git`'s performance tests on VIM3s (ARM64). This
    is the perfcompare version of the `terminal.vim3-release`
    builder. Note that `terminal.vim3-release` is not run by the CQ by
    default, so it is more likely to be broken or have higher flake
    rates than other builders.

### For integration.git CLs

**Perfcompare is not supported yet for `integration.git` CLs.**

Specifically, CLs that change dependencies in Jiri manifest files or
`jiri.lock` files or that use `patches.json` are not yet supported by
perfcompare. This includes CLs that change prebuilt packages, such as
toolchain roll CLs.

Perfcompare does not know how to check out the source and prebuilt
binaries before and after the CL in these cases, so it will give wrong
results in these cases. It will produce a finding that there is no
change in performance, even if the CL does change performance.

## Example output

Here is part of the output from a perfcompare run on a simple [test
CL]:

[test CL]: <https://fuchsia-review.googlesource.com/c/fuchsia/+/482567>

```none
Summary counts:
  2939 test cases in total
  2938 test cases had no significant difference (no_sig_diff)
  1 test case got faster
  0 test cases got slower
  0 test cases added
  0 test cases removed

Results from test cases with differences:

Test case                                 Improve/regress?  Factor change  Mean before         Mean after
----------------------------------------  ----------------  -------------  ------------------  -----------------
fuchsia.microbenchmarks: ExampleNoOpLoop  faster            0.143-0.145    405.36 +/- 0.39 ns  58.49 +/- 0.30 ns

Results from all test cases:

Test case                                      Improve/regress?  Factor change  Mean before        Mean after
---------------------------------------------  ----------------  -------------  -----------------  -----------------
...
fuchsia.microbenchmarks: Syscall/ManyArgs      no_sig_diff       0.986-1.008    92.94 +/- 0.66 ns  92.65 +/- 0.40 ns
fuchsia.microbenchmarks: Syscall/Null          no_sig_diff       0.993-1.007    84.33 +/- 0.40 ns  84.31 +/- 0.19 ns
fuchsia.microbenchmarks: Thread/CreateAndJoin  no_sig_diff       0.950-1.034    34229 +/- 711 ns   33935 +/- 739 ns
fuchsia.microbenchmarks: TicksGet              no_sig_diff       0.981-1.022    19.77 +/- 0.19 ns  19.81 +/- 0.21 ns
...
```

## Interpreting the results

*   `no_sig_diff` means that no [statistically
    significant](https://en.wikipedia.org/wiki/Statistical_significance)
    difference was found in the metric.  It does not mean that there
    was no difference, just that any difference was too small
    (relative to the amount of variation in the metric) to be
    detected.

*   `ci_too_wide` is shown in the "Factor change" column if the
    confidence intervals in "Mean before" and "Mean after" are so wide
    that the lower bound went negative.  This will happen for a metric
    if it has a large amount of variation.

## Testing CL stacks versus individual CLs

The perfcompare builder measures the performance impact of individual
CLs, **not** stacks of CLs.

As an example, suppose you have a series of CLs: P1, P2, P3, P4, P5,
where P1 is the oldest (that is, all the other CLs depend on it). If
you run perfcompare on P3, the "with CL" build will include P1+P2+P3,
while the "without CL" build will include just P1+P2.

*   This provides a way to measure effects on test cases that haven't
    been landed yet. You can have one CL that adds a new performance
    test, and a follow-on CL that changes the
    software-under-test. Running perfcompare on the second CL will
    show how that CL affects the new test.
*   If you do want to measure the overall effect of a patch stack, one
    way to do that is to squash the changes into a single Git commit
    (such as with `git merge --squash`), upload that to Gerrit, and
    run perfcompare on that.

### The "with CL" and "without CL" builds

The perfcompare builder applies the following steps sequentially to
produce the "with CL" and "without CL" builds:

1.  Check out Fuchsia from the current tip-of-tree revision of
    `integration.git`.
2.  Apply the CL series to the checkout, up to and including the CL
    being tested. This uses `jiri patch`, which uses `git rebase`.
3.  Build Fuchsia. This gives the "with CL" build.
4.  Unapply the topmost CL from the checkout (leaving earlier CLs in
    the CL series, if any, applied). This works by running `git
    checkout HEAD^` in the Git repo where the CL series was applied.
5.  Build Fuchsia again, doing an incremental build. This gives the
    "without CL" build.

Steps 1-3 are the same as for non-perfcompare Fuchsia try builders.

## Limitations

*   CLs that use `patches.json` or that change dependencies in Jiri
    manifest files are not supported yet, as mentioned above.

## How to run performance comparisons locally

The perfcompare builders use
[`perfcompare.py`](/src/testing/perfcompare/perfcompare.py) to compare
performance results. It is possible to use `perfcompare.py` to run
performance tests locally (that is, not using Fuchsia Infra) and
compare their results. See the
[documentation](/src/testing/perfcompare/README.md).

## How to download the raw performance results

<!-- Allow lines to be wrapped in the code blocks below. For single
     shell commands, wrapping can be more convenient for
     copy-and-pasting than using backslashes for splitting up
     lines. -->
<style>
pre.wrapped {
  white-space: pre-wrap;
}
</style>

It is possible to download the raw performance test results produced
by a perfcompare try builder run. This is useful if you want to modify
the analysis that `perfcompare.py` performs. To do that, use the
following steps:

1.  Find the values of the `cas_instance` and
    `perfcompare_dataset_digest` fields from the output properties of
    the perfcompare build. These can be found on the build page for
    the build (which is reachable from the "Checks" tab in the Gerrit
    code review). Examples of typical values are:

    *   `cas_instance="projects/chromium-swarm/instances/default_instance"`
    *   `perfcompare_dataset_digest="3ff389154e02490f29e379564f7e70b3df66f74c3116ed50172cceec1e9d9888/165"`

    For downloading results data from non-perfcompare builds, the
    field name to use is `perf_dataset_digest` rather than
    `perfcompare_dataset_digest`.

2.  Download the dataset by running the following command (using the
    prebuilt `cas` tool from the Fuchsia checkout):

    ```shell {:.wrapped}
    ./prebuilt/tools/cas/cas download -cas-instance $CAS_INSTANCE -digest $DIGEST -dir $DEST_DIR
    ```

3.  Run `perfcompare.py` on the downloaded dataset:

    ```shell {:.wrapped}
    python3 src/testing/perfcompare/perfcompare.py compare_perf $DEST_DIR/without_cl/ $DEST_DIR/with_cl/
    ```

Note that the RBE-CAS system keeps the data for only about 2-3 months,
so the download command will fail if the build was not run
recently. (The current default for the time-to-live (TTL) in
RBE-CAS is 90 days.)


[internal-doc]: <https://goto.google.com/fuchsia-perfcompare-internal>
