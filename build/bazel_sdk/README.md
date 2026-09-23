# Fuchsia Bazel SDK

This directory contains the Fuchsia Bazel SDK Rules and their tests.

Here is what each subdir contains:

- `bazel_rules_fuchsia`: build rules that get release as `rules_fuchsia` as well
   as the templates which generate the build rules that are released as the
   `fuchsia_sdk`.
- `e2e`: e2e tests that validate the SDK. See `e2e/README.md`.
- `tests`: unit tests that validate the SDK.

## Using a locally built SDK

Using a locally-built Bazel SDK requires two steps.

1. Building the SDK locally
1. Overriding the `fuchsia_sdk` and `rules_fuchsia` repositories in your local checkout

### Building the SDK

The Bazel SDK that is shipped to users is composed of two bazel repositories. The
`rules_fuchsia` repository is the static rules that are loaded by users and the
`fuchsia_sdk` which contains the generated BUILD rules for the contents of the
IDK. The process of generating this final artifact is driven by the GN build system since
the core IDK is still created by GN. In order to build the bazel SDK you must
invoke a gn build via fx.

```bash
fx build //sdk:final_fuchsia_sdk
```

NOTE: By default this target will build the Bazel SDK with for the all Supported
API levels and CPU architectures.

If you would like to build support for a subset of Supported API levels and CPU
arechiectures, you can override GN args to specify a list of each to build. For
example:

```
idk_buildable_api_levels = [26, "NEXT"]
idk_buildable_cpus = [ "x64" ]
```

Running this command will create the Fuchsia SDK and run the generators that
create the Bazel SDK.

The output of the build can be found by running the following command from the root
of the repository. This path is relative to the current out directory.
`build/api/client print bazel_sdk_info | select(.name == "final_fuchsia_sdk") | fx jq  '.[] .location'`

### Overriding @fuchsia_sdk and @rules_fuchsia

Once the SDK is locally built you can override your project's `@fuchsia_sdk//` and
`@rules_fuchsia//`  repositories with the one that is built locally by using Bazel's
`--override_repository`.

It can be helpful to put the path in an environment variable and create an alias
since this needs to be pass to each invocation of bazel.

```bash
export FUCHSIA_SDK_PATH="$(fx get-build-dir)/$(${FUCHSIA_DIR}/build/api/client print bazel_sdk_info | fx jq -r '.[]  | select(.name == "final_fuchsia_sdk") | .location')"
export RULES_FUCHSIA_PATH="$(fx get-build-dir)/$(${FUCHSIA_DIR}/build/api/client print rules_fuchsia_info | fx jq -r '.[] .location')"
export SDK_OVERRIDE=("--override_repository=fuchsia_sdk=$FUCHSIA_SDK_PATH" "--override_repository=rules_fuchsia=$RULES_FUCHSIA_PATH")
```

Then you can use the $SDK_OVERRIDE variable in all of your subsequent bazel
invocations

```bash
bazel build "${SDK_OVERRIDE[@]}" //foo:pkg
bazel test "${SDK_OVERRIDE[@]}" //foo:test
```

### Iterating on build rules

If you need to make changes to the SDK content, for example changing a fidl
file, you must recreate the Bazel SDK by running the commands listed above.
However, if you are just iterating on the starlark rules that make up the SDK,
you do not need to regenerate the SDK since these files are static. You can
simply make the change to the files and then trigger a new build.

## Using an infra-built Bazel SDK in your workspace

An alternate method for testing a Bazel SDK change is to leverage the Bazel SDK
LSC process on top of a CL.
With this approach you can help remove the guesswork around local build
configuration correctness by performing the following:

1. Upload your test changes in a CL.
2. Run the LSC process. You can either:
    1. Select "Choose Tryjobs" on the gerrit page and select a builder that
       matches the following regex: `sdk-bazel-linux-.+(?<!android|subbuild)$`
    2. Run `fx lsc <cl_url> bazel_sdk`
3. Wait for the LSC builder to finish.
4. Click into the LSC builder. This will open a new builder page.
5. Click into the "run external tests" step.
6. Click into the "gerrit_link". This will open a new gerrit page.
7. Copy the contents of `patches.json` locally into your OOT repo's root.

## Executing E2E Developer Workflow Tests

The `e2e` subdirectory contains a suite of tests for validating that an existing
developer workflow continues to work with the given SDK. More information on how
it works can be found in `e2e/README.md`.
