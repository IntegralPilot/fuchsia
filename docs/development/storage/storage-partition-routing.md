# Routing a storage partition to a component

While most components interact with storage through a filesystem using their
storage capabilities, certain components may need to be able to directly read
and write a partition.  This guide explains how to route a partition to a
component.

The steps, at a high level, are:

1. **Define a semantic label in the board configuration**:  Define a mapping of
   a physical partition label to a "semantic" label, which will be the name of
   the partition used in capability routing.
2. **Update component manifests**: Routing the directory capability for the
   block device to the component.
3. **Access the block device in the component**: The component uses the routed
   directory capability to access the block device.

## Steps

### 1. Define a semantic label in the board configuration

The first step is to declare a semantic label for the physical partition in your
board's configuration, which is typically a `.bazel` or `.bzl` file located in
`//boards/` or a vendor-specific equivalent.

In the board's configuration, add a `block_devices` list to the `filesystems`
dictionary. Each entry in this list is a dictionary that defines a new named
device.

* `device`: The semantic label you will use to refer to this partition in
   component manifests.
* `from`: Specifies where the partition should be found on the disk.
  + `label`: The GPT label of the partition to be routed.
  + `parent`: The parent device, which is usually "gpt".

**Example**: In your board's `BUILD.bazel` file:

```python
# In a board definition
fuchsia_board_configuration(
    name = "my_board",
    ...
    filesystems = {
        "block_devices": [
            {
                "device": "my_partition",
                "from": {
                    "label": "my-partition-label",
                    "parent": "gpt",
                },
            },
            {
                "device": "my_other_partition",
                "from": {
                    "label": "my-other-partition-label",
                    "parent": "gpt",
                },
            },
        ],
        ...
    },
    ...
)
```

This configuration is provided to `fshost` , which will match for GPT partitions
with the label "my-partition-label" and provide them at "/block/my_partition"
(and similarly for my-other-partition-label).

### 2. Route the Block Device to Your Component

Once the board is configured, you must route the capability for the block device
from `fshost` down to your component. This is done by adding `offer` stanzas to
the component manifests ( `.cml` files) of the parent components in the
topology.

The capability is a `directory` provided by `fshost` under the path `/block` .
The specific partition is selected by using the `subdir` parameter in the `offer`
stanza, which must match the `device` name you defined in the board
configuration.

In this example, we are routing a partition to a component in the `core` realm,
but steps are similar for other realms.  Modify the core shard for the component
to route the partition to it:

```cml
// my_component.core_shard.cml
{
    offer: [
        {
            directory: "block",
            from: "parent",
            to: "#my_component",
            subdir: "my_partition",
            as: "my_partition",
        },
        {
            directory: "block",
            from: "parent",
            to: "#my_component",
            subdir: "my_other_partition",
            as: "my_other_partition",
        },
        ...
    ],
    ...
}
```

### 3. Use the Block Device in Your Component

The final step is for your component to `use` the routed directory capability.

In your component's manifest ( `my_component.cml` ), add a `use` stanza for the
directory.

**Example**: `my_component.cml`

```cml
{
    use: [
        {
            directory: "my_partition",
            rights: [ "r*" ],
            path: "/block/my_partition",
        },
        {
            directory: "my_other_partition",
            rights: [ "r*" ],
            path: "/block/my_other_partition",
        },
        ...
    ],
}
```

Your component can now access the partitions in its namespace at `/block` .  To
connect to the `fuchsia.storage.block.Block` protocol for a given
partition, you connect via this path:

```rust
use fidl_fuchsia_storage_block::BlockMarker;
use block_client::RemoteBlockClient;

async fn connect_to_my_partition() -> Result<(), anyhow::Error> {
    let proxy = fuchsia_component::client::connect_to_protocol_at::<BlockMarker>(
        "/block/my_partition/fuchsia.storage.block.Block"
    )?;
    let block_client = RemoteBlockClient::new(proxy).await?;
    // ... use the block client
    Ok(())
}
```
