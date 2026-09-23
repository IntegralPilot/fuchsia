# Manifests

This directory contains Jiri
[manifests](https://fuchsia.googlesource.com/jiri/+/HEAD/manifest.md)
that list the source code and prebuilt dependencies of Fuchsia that get
downloaded by `jiri update`.

## Making changes

Many of these manifests are updating automatically by "rollers" - scheduled jobs
that create and submit CLs.

If making a change manually, you can test the impact of the change locally by
running `jiri update -local-manifest-project=fuchsia`, and then using `fx build`
and the rest of your normal Fuchsia development workflow to test the change.

If making a change to a `<package>` entry in a manifest, you'll need to run
`manifests/update-lockfiles.sh` to resolve the pinned versions to immutable
content addresses to ensure determinism.
