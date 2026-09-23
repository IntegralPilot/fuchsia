# Troubleshoot network connectivity: Device discovery

Once the network interface is properly configured, you can begin to probe for
the presence of the Fuchsia target device.

Note: The Fuchsia emulator (both AEMU and QEMU) uses different discovery
mechanisms, which are out of scope in this documentation.

The examples on this page use [`avahi`][avahi] to inspect mDNS records. The
`avahi` toolchain communicates with a local daemon, which may or may not be
running. If the daemon is not running, `avahi` commands will generally
fail with a message saying `Daemon not running`.

When needed, you can start a daemon as a foreground process. In a terminal,
run the following command:

```posix-terminal
sudo avahi-daemon
```

The daemon will cache mDNS queries as they are encountered, and it can be safely
killed when finished.

## Multicast DNS resolution {:#multicast-dns-resolution}

Device discovery uses multicast DNS (mDNS) to determine the IP address of the
Fuchsia target device. Fuchsia target devices broadcast their IP information to
mDNS with a service type of `_fuchsia._udp`.

To query this information with `avahi`, run the following command:

```posix-terminal
avahi-browse --resolve _fuchsia._udp
```

This command will appear to hang. It first dumps any records in the `avahi`
cache, and then sits and listens for mDNS broadcasts. If you use the `--resolve`
flag, `avahi` subsequently tries to resolve any records encountered. Resolution
yields the IPv6 address of the Fuchsia target device. For example:

```none {:.devsite-disable-click-to-copy}
$ avahi-browse --resolve _fuchsia._udp
+ zx-c863147051da IPv6 fuchsia-c863-1470-51da  _fuchsia._udp  local
= zx-c863147051da IPv6 fuchsia-c863-1470-51da  _fuchsia._udp  local
   hostname = [fuchsia-c863-1470-51da.local]
   address = [fe80::ca63:14ff:fe70:51db]
   port = [5353]
   txt = []
```

In this example, you can see that a host record was found that contains the
hostname of `fuchsia-c863-1470-51da` which then resolved to
`fe80::ca63:14ff:fe70:51db`. This is the address of the connected Fuchsia
target device.

## mDNS packet inspection {:#mdns-packet-inspection}

If no records are found, you can look to see if the target device is replying
to mDNS queries. mDNS operates on port 5353, which can be inspected with
`tcpdump`. When you run this command, restrict the command to your target
interface and filter the results by the IPv6 protocol and mDNS port. For
example:

```posix-terminal
sudo tcpdump -n -i zx-c863147051da "ip6 && port 5353"
```

This command prints output similar to the following, which shows an mDNS query
and the response cycle:

```none {:.devsite-disable-click-to-copy}
$ sudo tcpdump -n -i zx-c863147051da "ip6 && port 5353"
13:32:41.725007 IP 169.254.31.167.58994 > 224.0.0.251.5353:
    0 PTR (QU)? _fuchsia._udp.local. (37)
13:32:41.725057 IP 169.254.31.167.58994 > 224.0.0.251.5353:
    0 PTR (QU)? _fastboot._tcp.local. (38)
13:32:43.724131 IP6 fe80::ca63:14ff:fe70:51da.34810 > ff02::fb.5353:
    0 PTR (QU)? _fuchsia._udp.local. (37)
13:32:43.724183 IP6 fe80::ca63:14ff:fe70:51da.34810 > ff02::fb.5353:
    0 PTR (QU)? _fastboot._tcp.local. (38)
13:32:43.725635 IP6 fe80::ca63:14ff:fe70:51db.5353 > fe80::ca63:14ff:fe70:51da.34810:
    0*- [0q] 1/0/3 PTR fuchsia-c863-1470-51da._fuchsia._udp.local. (152)
```

## mDNS firewall and netfilter rules {:#mdns-firewall-and-netfilter-rules}

In some cases, you may be a need to check the kernel’s [`netfilter`][netfilter]
rules to discover a device through mDNS.

### ufw rules {:#ufw-rules}

`ufw` is a utility for managing the kernel's netfilter rules. If you have this
installed on your machine, you can check that your firewall is properly set up,
run the following:

```posix-terminal
sudo ufw status
```

This should return something similar to the following:

```none {:.devsite-disable-click-to-copy}
Status: active

To                         Action      From
--                         ------      ----
Anywhere (v6)              ALLOW       fe80::/10 5353/udp         # Fuchsia MDNS
33331:33340/udp            ALLOW       fe80::/10                  # Fuchsia Netboot Protocol
8083/tcp                   ALLOW       fe80::/10                  # Fuchsia Package Server
Anywhere (v6)              ALLOW       fe80::/10 33340/udp        # Fuchsia Netboot TFTP Source Port
33331:33340/udp            ALLOW       fc00::/7                   # Fuchsia Netboot Prot
ocol
8083/tcp                   ALLOW       fc00::/7                   # Fuchsia Package Server
Anywhere (v6)              ALLOW       fc00::/7 33340/udp         # Fuchsia Netboot TFTP Source Port
Anywhere (v6)              ALLOW       fc00::/7 5353/udp          # Fuchsia MDNS
```

If this isn't the case (i.e. the output is empty), run the following command:

```posix-terminal
fx setup-ufw
```

Running this command sets up several UFW rules for netfilter.

### netfilter rules

There may also be additional netfilter restrictions in place.

#### firewalld {:#firewalld}

To check the rules that are running on your machine, run the following:

```posix-terminal
sudo nft list tables
```

You may need to adjust rules if the output looks like the following:

```none {:.devsite-disable-click-to-copy}
$ sudo nft list tables
table ip filter
table ip nat
table inet firewalld
```

Most setups will likely have the first two rules:

```none {:.devsite-disable-click-to-copy}
table ip filter
table ip nat
```

To remove the `firewalld` rule, you can run the following commands:

```posix-terminal
sudo systemctl stop firewalld
sudo systemctl disable firewalld
sudo nft delete table inet firewalld
```

mDNS traffic occurs on port 5353 over the `224.0.0.251` and `fe02::fb` multicast
addresses (IPv4 and IPv6 respectively). It may be worth inspecting the `filter`
table, looking for any policy applied to port 5353 or these addresses. For
example:

```none {:.devsite-disable-click-to-copy}
$ sudo nft list table ip filter
table ip filter {
    ...
}
```

### Additional netfilter checks

If you've tried the above and the following is true:

* You do not have `ufw` installed on your machine. See [ufw rules](#ufw-rules).
* You do not have `firewalld` running on your machine. See
  [firewalld](#firewalld).
* You are not able to resolve the device's address with `ffx target list`.
* You _are_ able to resolve the device's address with `avahi`. See
  [Multicast DNS resolution](#multicast-dns-resolution).

You may still have some firewall rule blocking unicast mDNS messages. You may
need to add exceptions `iptables` manually in order to allow mDNS for link-local
IPv6 messages:

```posix-terminal
sudo ip6tables -A INPUT -p udp -s fe80::/10 --sport 5353 -m comment --comment 'Fuchsia MDNS' -j ACCEPT
sudo ip6tables -A INPUT -p udp -s fc00::/7 --sport 5353 -m comment --comment 'Fuchsia MDNS' -j ACCEPT
```

If this does not work, you may want to observe what kinds of firewall rules
might be affecting IPv6 traffic and address them accordingly:

```posix-terminal
sudo iptables -L -n
```

## Multicast ping {:#multicast-ping}

Another option to discover the target’s IPv6 address is to ping the local
multicast `ff02::1` device address. For example:

```posix-terminal
ping6 ff02::1%zx-c863147051da
```

This is a predetermined address specified by IANA. For more information, see
[section 2.7.1 of ch 2.7.1 of RFC-4291][rfc-4291].

Note that the local network interface was designated using the `scope-id`
component of an IPv6 address, which is a requirement of pinging a multicast
address. The `-I` flag could have been used to similar effect. Ping responses
come from the Fuchsia device and include their IP address information. For
example:

```none {:.devsite-disable-click-to-copy}
$ ping6 ff02::1%zx-c863147051da
PING ff02::1%zx-c863147051da (ff02::1%zx-c863147051da) 56 data bytes
64 bytes from fe80::ca63:14ff:fe70:51da%zx-c863147051da: icmp_seq=1 ttl=64 time=0.088 ms
64 bytes from fe80::ca63:14ff:fe70:51db%zx-c863147051da: icmp_seq=1 ttl=64 time=0.958 ms
```

Be aware that the localhost may itself reply to the multicast ping. In this
example, `fe80::ca63:14ff:fe70:51da` is the address of the host-side interface
(as determined above), and `fe80::ca63:14ff:fe70:51db` is the address of the
Fuchsia target device.

A good rule of thumb is that the device responding in far less than a
millisecond is generally the localhost responding, and anything approaching one
or more milliseconds to respond is likely the Fuchsia target device.

<!-- Reference links -->

[avahi]: https://avahi.org/
[netfilter]: https://www.netfilter.org/
[rfc-4291]: https://datatracker.ietf.org/doc/html/rfc4291#section-2.7.1
