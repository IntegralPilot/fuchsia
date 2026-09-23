# bt-le-broadcast-assistant
`bt-le-broadcast-assistant` is a tool for acting as an LE broadcast assistant.

## Usage

Include the tool in your `fx set` command by appending `--with "//src/connectivity/bluetooth/tools:core"`.

```sh
$ bt-le-broadcast-assistant [--use-static-address]
```

Upon starting the program, an interactive REPL will be launched.

### Commands

Type `help` in the REPL to see a full list of available commands.
You can always run `SOME-COMMAND -help` to see what arguments a subcommand accepts.

### Setup for testing

Pair to a LE audio headset so that you are ready to test the `bt-le-broadcast-assistant` tool.

**Terminal 1:**

```sh
$ fx shell
$ bt-cli
$ bt> allow-pairing confirmation display
```

**Terminal 2:**

```sh
$ fx shell
$ bt-cli
$ bt> start-discovery
$ bt> list-peers

# Find the <peer-id> of the headset that you're looking for.
$ bt> connect <peer-id>
```

Going back to **Terminal 1**, you should see the pairing request. Follow the prompt on the screen to accept the pairing.

Example output in **Terminal 1**:

```sh
Pairing request from peer: Peer FooBar ([address (public) XX:XX:XX:XX:XX:XX])
Accept? (y/n): y
Accepted pairing
Pairing complete for peer (id: <peer-id>, status: success)
Completed successful pairing with xxxxxxxxxxxxxxxx.
<peer-id> [address (public) XX:XX:XX:XX:XX:XX] [bonded]
<peer-id> [address (public) XX:XX:XX:XX:XX:XX] [connected]
```

In **Terminal 2**, you shall see that the peer is now bonded and connected.
**Going back to `bt-cli` in Terminal 2, disconnect the peer.** This step is needed in order for the peer to appear in `bt-le-broadcast-assistant` scan:

```sh
# Messages from pairing succeed
<peer-id> [address (public) XX:XX:XX:XX:XX:XX] [bonded]
<peer-id> [address (public) XX:XX:XX:XX:XX:XX] [connected]

# Disconnect so that the peer appears in `bt-le-broadcast-assistant` scan.
$ bt> disconnect <peer-id>
<peer-id> [address (public) XX:XX:XX:XX:XX:XX] [disconnected]
```

### Testing

Once you've done the preparation, you can now use the `bt-le-broadcast-assistant` tool to test the LE broadcast assistant functionality.

```sh
$ fx shell
$ bt-le-broadcast-assistant [--use-static-address]
$ ASSISTANT> help

# This should print out the peer ID for the scan delegator (aka LE audio headset) peer.
ASSISTANT> scan 5
ASSISTANT> connect <scan-delegator-peer-id>

# This should print out the known broadcast sources.
ASSISTANT> info

ASSISTANT> add-broadcast-source <broadcast-source-peer-id>
ASSISTANT> set-peer-addr <broadcast-source-peer-id> YY:YY:YY:YY:YY:YY Random

# Here N is the number of BIGs the broadcast source has.
ASSISTANT> force-discover-empty-source-metadata <broadcast-source-peer-id> N

ASSISTANT> add-broadcast-source <broadcast-source-peer-id> PaSyncNoPast
```

If the broadcast source was successfully added, you should see a printout similar to this:

```
ASSISTANT>      [BASS Event] AddedBroadcastSource(BroadcastId(15772355), NotSynced, NotEncrypted)
```

Explore other commands like `set-broadcast-code`, `remove-broadcast-source` and `update-pa-sync` to test other functionalities.
Note that some commands accept `<broadcast-source-id>` instead of `<broadcast-source-peer-id>`.

## Unit test

Use the below `fx` commands to run unit tests for this tool:

```sh
$ fx add-test
$ fx test bt-le-broadcast-assistant-unittests
```
