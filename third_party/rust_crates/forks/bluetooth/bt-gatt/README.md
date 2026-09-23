# bt-gatt

An abstraction layer enabling Bluetooth Low Energy Central and Peripheral
crates to be built in a Bluetooth host stack agnostic way.

Crates that wish to interact with GATT Services or implement a Service should
use this crate, and accept or generalize on the `bt_gatt::Central` or
`bt_gatt::Peripheral` trait implementations, which will be provided by a
Bluetooth stack support Crate.

An example server and client for the Battery Service is provided.

Implementations for the following Bluetooth stacks exist in other crates, i.e.:
 - bt-gatt-sapphire
 - bt-gatt-fluoride

## Examples

### Connecting from a central to a service

```rust
async fn print_volume_changes(central: impl bt_gatt::central::Central) -> Result<(), Error> {
    let peers_with_vcs = central.scan(VolumeControlService::UUID.into());
    // Presumably we would match more than just the first peer at some point.
    let Some(first_match) = matches.next().await else {
        panic!("Stack shutdown before we found a peer");
    };
    let client = central.connect(first_match.id).await?;

    let handle = client.find_service(VolumeControlService::UUID.into()).await?;
    let service = handle.connect().await?;

    let current_char = service.discover_characteristics(Some(0x2b7d_u16.into())).await?.pop().unwrap();
    let mut current_state: [u8; 3] = [0; 3];
    let _ = service.read_characteristic(current_char.handle, 0, &mut current_state[..]).await?;

    let updates = service.subscribe(current_char.handle);

    while let Some(Ok(notif)) = updates.next().await? {
        let new_state = notif.value;
        let muted = new_state[1] == 1;
        let muted_str = if muted { " muted" } else { "" };
        print!("New volume: {}{}", new_state[0], muted_str);
    }
}
```

# Stack Connector Crates

Bluetooth stacks that wish to make use of the crates that abstract over bt-gatt to provide services or clients should provide a method to
use the core traits:
 - [`central::Central`] enables scanning and connecting to peers, providing connections to [`client::Client`]
 - [`server::Server`] enables publishing Services and accepting connections from peers
 - [`peripheral::Peripheral`] enables advertising

