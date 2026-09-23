# aml-hrtimer

Driver providing high resolution timers support for the AMLogic A311D SoC.
These timer hardware is not part of the main CPU which is managed by the kernel.

## Testing

Unit tests for aml-hrtimer are available in the `aml-hrtimer-tests`
package.

```
$ fx test aml-hrtimer-tests
```

