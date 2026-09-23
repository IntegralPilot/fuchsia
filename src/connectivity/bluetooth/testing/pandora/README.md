# Pandora testing ecosystem integration

Spin up a gRPC server implementing the [Pandora APIs](https://developers.google.com/pandora/guides/bt-test-interfaces/overview).
For instructions on how to run Pandora tests with Rootcanal such as PTS-bot, visit [go/configure-sapphire-for-pandora](https://docs.google.com/document/d/1XwRLCjhFrHrsp5HSFZ38xU_4TAt-g27Vxie7XDcTTto/edit?usp=sharing&resourcekey=0-pF_J5m3CzcN36JiUNl3mew).

Until first-party Rust gRPC bindings are available, the Pandora gRPC server is
a C++ scaffolding that calls into the C FFI bindings to the bt-affordances Rust
library which handles the interactions with Sapphire.