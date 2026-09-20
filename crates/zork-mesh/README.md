# Zork Mesh library integration

The Station owns `managed::Runtime` and shares its `MeshNode` handle with
product and enrollment services. `managed::start` acquires the Synch lifecycle
lock, initializes existing-or-new node state, calls `synch_engine::Node::open`,
and starts the engine loops on the caller's Tokio runtime. The Station awaits
background failure and clean shutdown. Supervisor manages the Station, which
embeds the Agent; it does not open or supervise a Synch node.

`MeshNode` calls typed Rust APIs for trust, delegation, source publication,
verified content reads and socket connections. There is no `synch-cli`, local
gRPC client/server, control socket/token, protobuf code generation or command
interpreter. A stopped library handle cannot operate on the node; releasing
its engine reference also allows immediate rebinding of the same UDP port.

The desktop's remote transport uses the same library API on its background
executor. Its shared handle is explicitly rebound after network settings
change; no transport helper is launched.

Remote product RPC, subscriptions and service tunnels use bidirectional QUIC
streams on the native `zork-control/1` ALPN. The authenticated iroh peer identity
and local Mesh membership govern access. There is no eBPF control bridge or
fallback to the former socket protocol; peers must use the native transport.

Synch is pinned to v0.1.8, Git revision
`6d6283f09c32476dc77c09f76a2b2529a42a558d`, in Cargo.toml and Cargo.lock.
The dependency's license is retained in `LICENSE.synchronicity`.

Upgrading from the former daemon/supervisor-owned implementation requires a
full node stop/start so the previous owner releases the identity lock. Starting
a second owner is rejected; the library does not kill a process to take over.
The identity/database/CAS directory is reused. Obsolete control socket/token
files are removed only after acquiring exclusive ownership.
