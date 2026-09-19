# Relay credential replacement

This is the crates.io source of iroh 1.0.3, upstream commit
`f2eb930dda3779c6d852b72f3712aacd6e573ab1` (the `iroh` directory).
The original licenses, manifest, source and tests are retained.

The local patch makes a relay-map update retire an active relay actor when its
bearer credential changes or its configured relay is removed. The replacement
actor reads the current config. Upstream only schedules address discovery on
these updates, so the existing actor's ClientBuilder otherwise keeps using the
old credential, including after logout and a subsequent login.

Only relay admission lifecycle changes. Discovery, direct transports, relay
selection, DNS and hole-punching policy remain upstream behavior. Native
credential integration is exercised through Zork's CLI, Station and the Worker
by `deploy/cloudflare/test/native-lifecycle.ts`.
