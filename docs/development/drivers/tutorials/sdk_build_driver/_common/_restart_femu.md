## Restart the emulator

Shut down any existing emulator instances:

```posix-terminal
ffx emu stop --all
```

Start a new instance of the Fuchsia emulator with driver framework enabled:

```posix-terminal
ffx emu start core.x64 --headless
```
