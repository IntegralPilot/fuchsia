# End to end tests

These are end to end tests for ffx.

* cli_compat is a compatibility test that checks that the current
command line arguments for all commands are compatible with a golden file set.

This test is written using the golden_file_test() GN template. If the golden files
need to be updated, the error message will provide the command to run to copy the
file from the output directory to the source directory. Alternatively, you can rebuild
setting the build arg `update_goldens=true`.

Example:

```
# Include cli-goldens in your build configuration

$ fx set minimal.x64 --with-host //src/developer/ffx/tests/cli-goldens:tests

# Edit your args.gn via fx args and include `update_goldens=true`

$ fx args # Add updated_goldens=true

# Since these are golden file tests implemented as GN actions, they are
# executed during the build process. You "run" them by building the target:

$ fx build --host //src/developer/ffx/tests/cli-goldens:tests
```

You will see some messages indicating the goldens need to be updated but since
`update_goldens=true` is set, you should see the golden files automatically
updated. These need to be checked in.
