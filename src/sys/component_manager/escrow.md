# Escrow design

This records an in-progress design of how a component can stop itself and stow
away some important state in the framework. This markdown should be moved to
standalone documentation as the feature is built out, and we may amend the
details. After we prove that we can stop three components, the full design will
be socialized as an RFC where the syntax and security implications etc. will be
refined.

> **Note:** Protocols need to be marked as `@discoverable` to support escrowing.
  This allows components to reconnect to channels after waking from idle.

## Escrowing stateless protocols

Some FIDL connections don't carry state. Every request functions identically
whether they are sent on the same connection or over separate connections.
Component authors should do the following:

- Declare the capability if not already. You may need to declare the capability
  if this protocol connection is derived from another connection, and is
  otherwise not normally served from the outgoing directory. E.g.
  `fuchsia.inspect.Tree` is derived from `InspectSink`.

- Add `delivery: "on_readable"` when declaring the capability. The framework
  will then monitor the `ZX_CHANNEL_READABLE` signal on the server endpoint of
  new connection requests, and connect the server endpoint to the provider
  component when there is a message pending. The framework will drop the server
  endpoint if the `ZX_CHANNEL_PEER_CLOSED` signal is asserted. If the connection
  request requires an `OnOpen` or `OnConnectionInfo` event to be sent, the
  framework will still eagerly forward to request to the component despite the
  `"on_readable"` declaration.

- Add a use declaration from `self` for the capability such that the program may
  connect to it from its incoming namespace.

- Typically, a component will wait until a stateless FIDL connection to
  momentarily have no pending replies nor unread messages. Then it will unbind
  the server endpoint, and pass it to the framework by opening the capability
  used in the previous step from its incoming namespace.

- When the server endpoint is readable again, the request may be delivered to
  the current execution of the component or to a future execution of the
  component, depending on whether the current execution of the component calls
  [`ComponentController.OnEscrow`] or have exited on its own.

- If errors happen when waiting and passing back the request, the errors will be
  logged in the component's LogSink.

- We might add some syntax sugar for this pattern in the future, maybe an
  "escrow" block in the component manifest, to clarify the intent. Alternatively
  a protocol could be marked `escrow: "true"` which implies
  `delivery: "on_readable"` and also add an entry in the incoming namespace. We
  could also build client libraries that discourage accidental misuse of the
  escrow (such as connecting to a protocol that is not declared
  `delivery: "on_readable"`).

## Escrowing stateful protocols, and other important state

Other things do not map cleanly to a connection into the outgoing directory.
Notably the outgoing directory server endpoint that a component has obtained on
startup itself cannot be sent into the outgoing directory client endpoint.

The component should put those in
`fuchsia.component.runner/ComponentController.OnEscrow`. ELF components can do
that via `fuchsia.process.lifecycle/Lifecycle.OnEscrow`, which the ELF runner
will proxy accordingly.

## Handling component updates

The framework will discard the escrow state if the component is being
unresolved. This is to ensure that the next version of the component do not get
requests intended for the previous version of the component.

Drawback: long-living connections will then be forcefully closed during an
upgrade. The alternative is that we introduce ABI compatibility considerations
between the previous and next version of a component. The next version could
declare a capability at a different path and we would get an error unless care
is exercised when upgrading components.

