# Developer console

The developer console is a realm existing in the `core` realm where capabilities
are routed to for use by console tools. It provides a centralized place to route
capabilities needed by interactive development and direct target access.

Within the `developer-console` realm, a single child (`launcher`) exposes the
`fuchsia.developer.console.Launcher` protocol that can be routed out to agents
starting developer shells like `ffx` and `sshd-host`.
