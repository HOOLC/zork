Upstream: https://github.com/n0-computer/iroh, crates.io iroh 1.0.3.

Package SHA-256: `460de6bc52163b41b1646931f2897e5ab986f0966ade444467fec25024751a72`.

Local changes permit separate QUIC address-discovery servers and their DNS resolver, excluding those servers from relay selection, and carry the endpoint HTTPS proxy into network probes. Transport identity, TLS verification and hole-punching remain the upstream protocols.

Relay configuration changes also retire actors holding an obsolete bearer credential, so logout and token replacement reconnect with current access. This patch was received from the relay-account-lifecycle task; it leaves direct transports and other relay actors intact.

Explicitly withdrawn configured origins remain unavailable until reinserted. Stale peer discovery must not recreate their relay actors without credentials after logout; unrelated peer relays retain their upstream behavior.
