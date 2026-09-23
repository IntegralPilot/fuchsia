This test exercises the various services offered by `memory_monitor2`:
- `ffx profile memory component`
- performance traces
- inspect data

Test it locally with:

```
fx set workbench_eng.x64 --with-host //src/performance/memory/attribution/monitor/tests/e2e:tests
fx build
fx ffx emu start -H
fx serve
```

In another terminal:

```
rm -rf /tmp/test/* ; fx test --e2e --output --simple --ffx-output-directory /tmp/test //src/performance/memory/attribution/monitor/tests/e2e:memory_monitor2_e2e_test
```

Quit this one with `ctrl-a`, `x`
All outputs can be found in `/tmp/test`.
