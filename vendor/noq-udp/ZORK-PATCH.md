# Android UDP segmentation compatibility

Source: `noq-udp` 1.2.0 from crates.io, upstream [n0-computer/noq](https://github.com/n0-computer/noq/tree/a62dafd56ad9f90e759e0c3917176a5969871b2c/noq-udp).
The original archive SHA-256 is `b56d621ed2c1773b5356ab39cc68c819f8d0103f7493d8661c4a5f5f8ea55f40`; original licenses, manifest, and VCS metadata are retained.

The sole source change is in `src/linux.rs`: report one transmit segment on Android. Some Android paths advertise `UDP_SEGMENT` support but stall on the first multi-datagram send. This keeps QUIC framing, authentication, retransmission and flow control intact, while sending individual UDP packets. Linux and other platforms retain the upstream capability detection.

Reassess this patch when upgrading to a release containing the [upstream GSO retransmission fix](https://github.com/n0-computer/noq/pull/746). Removal requires Android interop with requests larger than the path MTU, including shared-tree metadata and full file transfer; a handshake or a single small request is insufficient.
