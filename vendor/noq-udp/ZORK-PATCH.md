# Android UDP segmentation compatibility

Source: `noq-udp` 1.2.0 from crates.io, upstream [n0-computer/noq](https://github.com/n0-computer/noq/tree/a62dafd56ad9f90e759e0c3917176a5969871b2c/noq-udp).
The original archive SHA-256 is `b56d621ed2c1773b5356ab39cc68c819f8d0103f7493d8661c4a5f5f8ea55f40`; original licenses, manifest, and VCS metadata are retained.

The sole source change is in `src/linux.rs`: report one transmit segment on Android. Some Android paths advertise `UDP_SEGMENT` support but stall on the first multi-datagram send. This keeps QUIC framing, authentication, retransmission and flow control intact, while sending individual UDP packets. Linux and other platforms retain the upstream capability detection.

Reassess this patch when upgrading to a release containing the [upstream GSO retransmission fix](https://github.com/n0-computer/noq/pull/746). Removal requires Android interop with requests larger than the path MTU, including shared-tree metadata and full file transfer; a handshake or a single small request is insufficient.

# Apple IPv4 source address pinning (Zork patch)

`src/unix.rs` no longer enables `IP_RECVDSTADDR` on Apple platforms and no longer encodes
`transmit.src_ip` for IPv4 there (`src/apple_fast.rs` likewise). XNU ignores
`IP_RECVDSTADDR` as a send control message (there is no `IP_SENDSRCADDR`); the kernel always
chooses the IPv4 source from the route. Verified on macOS 26: a datagram "pinned" to
192.168.20.10, 192.168.0.126 or a Surge TUN address 198.18.0.1 leaves with the routed source
(192.168.20.107 / 192.168.20.152) and the original port.

Reporting the receive destination anyway made noq key IPv4 paths by a local address the
outgoing packets do not carry. Observed failures: a multi-homed macOS server (two addresses
on one interface) binds the handshake to the address the first Initial hit, replies from the
routed address, and then discards the client's handshake packets sent there ("discarding
packet sent to incorrect interface"); behind a fake-IP proxy TUN the path is keyed to the TUN address.
With `dst_ip` unset, IPv4 paths on Apple are identified by the remote address only, which is
what the kernel actually provides. IPv6 keeps `IPV6_PKTINFO`, which XNU honours for sending.
Regression test: `apple_ipv4_path_has_no_local_ip` in `tests/tests.rs`.
