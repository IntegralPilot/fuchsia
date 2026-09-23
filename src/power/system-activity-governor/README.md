# System Activity Governor

## Building

To add this component to your build, append
`--with-base src/power/system-activity-governor`
to your `fx set` invocation.

## Testing

To add tests to your build, append `--with
//src/power/system-activity-governor:tests` to your `fx set` invocation.

Run tests with:

```
$ fx test system-activity-governor
```

To see output, use the `-o` flag. First adjust the `MACRO_LOOP_EXIT` constant
so that the assertion actually fires for non-matching Inspect.
