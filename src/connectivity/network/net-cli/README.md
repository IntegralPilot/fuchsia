# net-cli

`net-cli` is a CLI tool which can run directly on target or as an FFX
plugin for configuring and inspecting the Netstack.

## `capture` subcommand

The `capture` subcommand is used for managing packet captures on Fuchsia.

### Start a rolling capture

Start capturing on interface `lo` using the default capture name `capture`:
```bash
net capture start-rolling name:lo
```

Start capturing on interface ID `2` with a custom name `my_capture`, packet size
limit of 100 bytes, and buffer size of 4 MiB:
```bash
net capture start-rolling --name my_capture --snap-len 100 --capture-size 4194304 id:2
```

Start capturing with a tcp port filter:
```bash
net capture start-rolling name:lo "tcp port 80"
```

### Stop and download/discard

Stop the default capture and download to the default `/tmp/pcap/capture.pcapng`:
```bash
net capture stop-rolling
```

Stop a custom capture and download to a specific path:
```bash
net capture stop-rolling --name my_capture --output /data/my_capture.pcapng
```

Stop capture and discard the data immediately without downloading:
```bash
net capture stop-rolling --skip-download
```
