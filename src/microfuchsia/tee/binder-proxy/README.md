# binder-proxy

This component will be the Binder RPC proxy between the TEE and external clients
including the host VM and (one day) other VMs. The proxy exposes the
Binder RPC protocol IMicrofuchsia over virtio-socket on the port specified in
the binder definition (currently 5680). The host is expected to provide an
instance of IHostProxy over this interface to let the TEE send requests
to the host.

## TA Sessions

For each TA found in the configuration exposed to the binder-proxy the proxy
instantiates a Binder RPC server. The proxy allocates listening ports to TA
servers in iteration order starting one above the control port. The root
object of each server is an instance of ITrustedApp. The RPC server
connects to the FIDL protocol exposed by the TA lazily when a binder
call is received.
