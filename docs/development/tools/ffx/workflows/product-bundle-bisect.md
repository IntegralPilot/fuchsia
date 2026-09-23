# Bisect product bundles

The `ffx product-bundle bisect` command helps identify regressions in product
bundles by bisecting over assembly artifacts.

## Concepts

When a bug or behavior change occurs between two versions of a product bundle,
`ffx product-bundle bisect` can help pinpoint which specific artifact
(platform, board, or product) introduced the change. It does this by:

1. Generating a list of released versions between a known-good version
   (`--from-success`) and a known-bad version (`--to-failure`).
2. Assembling intermediate product bundles using combinations of artifacts from
   different versions.
3. Facilitating tests on these intermediate bundles to determine if they pass
   or fail.

By using a binary search (or median cut) approach over the artifacts, the tool
quickly narrows down the source of the regression.

## Usage

You can run the bisection manually by following the prompts from the tool.

### Manual Bisection

To start a bisection between two known versions of a product bundle, run:

```posix-terminal
ffx product-bundle bisect <name> --from-success <version> --to-failure <version>
```

Replace `<name>` with the name of the product bundle (e.g., `core.vim3`) and
`<version>` with the product bundle version numbers.

The tool will assemble intermediate bundles and prompt you to test them. After
testing, you inform the tool whether the step passed or failed, and it
calculates the next step.

#### Pausing and Resuming

You can pause the bisection process at any time by pressing `CTRL+C`. The
tool saves its state. The next time you run the exact same
`ffx product-bundle bisect` command, it will ask if you want to continue
the previous run or start over.

## Options

Detailed options for `ffx product-bundle bisect`:

-   `--from-success`: Known-good version of the product bundle.
-   `--to-failure`: Known-bad version of the product bundle.
-   `--slot`: Slot to bisect over (`a` or `r`). Defaults to slot `a`.
-   `--out-dir`: Directory to write assembled images and other artifacts.
    Defaults to `~/plan_directory/out`.
-   `--gen-dir`: Directory to write intermediate files. Defaults to
    `~/plan_directory/gen`.
-   `--auth`: Authentication method to use for fetching artifacts.

## Examples

Bisecting a product bundle manually:

Note: `core.vim3` is used as an example, but may have limited support
depending on your environment.

```posix-terminal
ffx product-bundle bisect core.vim3 \
    --from-success 29.20250826.6.1 \
    --to-failure 29.20250905.6.1
```
