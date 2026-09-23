## Start the emulator

Stop all emulator instance you may have currently running:

```posix-terminal
ffx emu stop --all
```

Start a new instance of the Fuchsia emulator with Driver Framework v2 enabled:

```posix-terminal
ffx emu start core.x64 --headless
```

Note: Driver Framework v2 is not enabled by default. The `--kernel-args` options
configure the emulator instance to use the latest driver framework.
