# Zork Mesh transport

Station and client hosts own `managed::Runtime` and share its `MeshNode` handle.
The runtime binds iroh directly for authenticated RPC, subscriptions and service
streams. It does not start a general file synchronization engine, directory
scanner, anti-entropy loop or separate transport daemon.

Attachment transfer names immutable content by source, hash and size. Both cached
and received bytes are verified, and cached remote bytes still require source
access. Product messages and operation receipts retain their own business stores
and synchronization cursors.

The runtime owns discovery, relay proxy IO and connection shutdown. Dropping the
host initiates cleanup; explicit shutdown waits before releasing identity ownership.
The existing lifecycle lock and device public key are preserved during migration.
The current runtime uses its own identity and immutable object store under
`mesh/iroh`. It does not read or migrate earlier Synch stores or protocols.
