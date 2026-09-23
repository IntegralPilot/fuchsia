# vsock-sshd-host

vsock-sshd-host listens on a vsock port (optional argument, default: 22) for
connections and spawns an sshd in inetd mode for each incoming connection.

sshd-host also executes creates new host keys during initialization and hosts
the generated keys in its outgoing directory for sshd to use.
