# ffx bluetooth Commands

## Peer
TODO

## Pandora
TODO

## Controller

### Help

List available commands.

### Show

Show the active controller.

```sh
# SUCCESS
$ ffx bluetooth controller show
HostInfo:
        identifier:     902a821cd04098fc
        addresses:      [address (public) DA:4C:10:DE:17:02]
                        [address (random) 60:AC:34:EA:8F:40]
        active: true
        technology:     DualMode
        local name:     fuchsia-emulator
        discoverable:   false
        discovering:    false
```
```sh
# FAIL
$ ffx bluetooth controller show
No host found.
```

### List

List all known bluetooth controllers.

```sh
# SUCCESS
$ ffx bluetooth controller list
HostId           Addresses                            Active Technology Name             Discoverable Discovering
8090d38dd13307d7 [address (public) DA:4C:10:DE:17:01] true   DualMode   fuchsia-emulator false        false
                 [address (random) 79:47:10:8A:8E:39]
```

```sh
# FAIL
$ ffx bluetooth controller list
No host instances detected
```