# Shush

This tool consumes json diagnostics emitted by the rust compiler (and clippy), and tries to address them by either inserting `#[allow(...)]` annotations or applying compiler-suggested fixes to the code automatically. It can be used to make large scale changes to roll out new clippy lints, perform edition migration, and address new compiler warnings from upstream.

## Usage:

First, ensure the tool is included in your build:

```sh
fx set ... --with-host //tools/shush:install
```

Then you can use `fx shush`:

``` sh
# Allow a specific lint or category (skip generating bugs)
fx clippy -f <source file> --raw | fx shush --api $API_PATH --mock lint --lint clippy::suspicious_splitn allow

# See all clippy lints in our tree
fx clippy --all --raw | fx shush --api $API_PATH --mock lint --lint clippy::all --dryrun --mock allow

# Manually specify a fuchsia checkout to run on
fx shush lint lint_file.json --lint clippy::style --fuchsia-dir ~/myfuchsia fix

# Run shush on itself
fx clippy '//tools/shush(//build/toolchain:host_x64)' --raw |
    fx shush lint --force --lint clippy::needless_borrow fix

# Automatically generate bugs while allowing
fx shush --api $API_PATH lint lint_file.json --lint clippy::absurd_extreme_comparisons allow

# Roll out generated bugs (do this after checking in allows)
fx shush --api $API_PATH rollout
```

Run `fx shush --help` for details.

## Performing a lint rollout

`shush` can perform large-scale lint rollouts across the tree automatically:

1. Create a tracking issue by hand. This can be done from the issue tracker UI. Keep the issue number for later.
2. Get an issue tracker API binary. This should accept a `bugspec` and respond to create, update, and list-components requests.
3. Pick a commit to generate codesearch links with. This should be a commit prior to the lint rollout CL.
4. Write an issue description template in Markdown. The file should contain `INSERT_DETAILS_HERE` where code links should be added. See `description_template.md` for an example.
5. `fx clippy --all --raw | shush lint --lint $LINT --api $API_PATH allow --codesearch_ref $COMMIT --template $TEMPLATE --blocking-issue $ISSUE`
6. Land a CL with the generated changes.
7. `shush --api $API_PATH rollout`

`shush allow` also takes additional arguments for customizing the generated issues, including CC limits and the component to create the issues in.
