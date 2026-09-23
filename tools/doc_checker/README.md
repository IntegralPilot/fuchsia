# Doc checker

The present tool verifies the structure and content of documentation files.

## What does it check?

Doc-checker checks the links and table of contents graph to enforce consistency
and that all markdown pages in //docs are reachable.
See more at <https://fuchsia.dev/fuchsia-src/contribute/docs/doc-checker>

## Usage

To reproduce the check run as a static check  for presubmits, Run:

```sh
fx doc-checker  --local-links-only
```

To get JSON encoded format of the output use `--json`

## What if there is a bug?

Please [file a bug](https://bugs.fuchsia.dev/p/fuchsia/issues/entry?components=DeveloperExperience%3EDocTools).
If it is blocking - let the OWNERS know and/or tq-devrel-eng@
and we'll work on it.

## What if I  have an idea of something to check?

Please file a bug. Or contribute here.
