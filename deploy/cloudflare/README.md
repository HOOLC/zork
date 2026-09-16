# Cloudflare Mesh services

Deploy an official **iroh-relay 1.1.0** container and a pkarr HTTP discovery service.
The relay protocol is compatible with Zork's iroh 1.0.3 dependency. Cloudflare
terminates public TLS; the relay falls back from TLS-exporter authentication to
its signed challenge handshake. Peer traffic remains encrypted end to end by iroh.

The Worker exposes:

| Path                            | Purpose                                                      |
| ------------------------------- | ------------------------------------------------------------ |
| `/relay`                        | Official iroh WebSocket relay, routed to one container       |
| `/pkarr/<z-base-32-public-key>` | Signed address record PUT and GET                            |
| `/healthz`, `/ping`             | Worker reachability, not an end-to-end relay readiness check |

## Deploy

Use Node.js 22+ and pnpm 10.33.0. Docker must support Linux amd64 images. The
Cloudflare account must have Workers/Containers available; resource usage is billed
by Cloudflare. The configuration caps the relay at one `lite` container. Active
connections keep the relay running; this is not a zero-cost idle Worker deployment.

```sh
cd deploy/cloudflare
npx --yes pnpm@10.33.0 install --frozen-lockfile
cp wrangler.jsonc wrangler.local.json
```

Edit the ignored `wrangler.local.json` to set `account_id`, an unused Worker name,
and `vars.ALLOWED_KEYS` (comma-separated lowercase 64-character hex endpoint public
keys). Set a custom domain with `routes: [{"pattern":"relay.example.com",
"custom_domain":true}]`. No private keys or invitation secrets belong in this file.

Include each device's Mesh identity, the desktop transport identity, and any
invitation/enrollment endpoint identity that needs to use this relay. An empty
allowlist denies all endpoints. Adding new devices therefore includes updating
this infrastructure allowlist; a Zork invitation alone does not grant relay access.

```sh
npx --yes pnpm@10.33.0 exec wrangler login --device
npx --yes pnpm@10.33.0 types
npx --yes pnpm@10.33.0 check
npx --yes pnpm@10.33.0 test
npx --yes pnpm@10.33.0 run deploy
```

`deploy.py` uses a temporary Docker configuration for the 15-minute registry token,
so SSH/background deployment does not require an unlocked macOS keychain. It retains
the current Docker host and CLI plugin paths, and deletes the temporary credentials
on exit. It does not change global Docker settings.

The relay's allowlist is read when its process starts. After changing allowed keys,
ensure the existing relay container has restarted before treating the change as
effective. Existing client connections will need to reconnect. Discovery checks
the current allowlist on every request.

## Client configuration

```json
{
  "relay_urls": ["https://relay.example.com"],
  "discovery_url": "https://relay.example.com/pkarr"
}
```

Apply this through the service configuration described in
[`docs/design/devices.md`](../../docs/design/devices.md). The running Station
needs its Mesh configuration updated separately. That operation restarts the
Station, so check for active tasks first. Preserve membership, grants and identities.
`offline: true` must be disabled for public services to work.

## Validation

The unit tests cover signed packets, tampering, timestamp bounds and body limits.
`test/http-smoke.ts` runs against a real local or remote Worker and checks signed
PUT/GET, retries, stale/conflicting writes, concurrent ordering and denied access.

For protocol interoperability, build the native probe from the repository root:

```sh
CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_BUILD_JOBS=4 \
  python3 scripts/lib/build_env.py -- cargo build --locked -p zork-mesh --example network-probe
```

Run the resulting `network-probe init <isolated-directory>` and put both printed
public keys in the allowlist. Then run:

```sh
network-probe check <isolated-directory> https://relay.example.com https://relay.example.com/pkarr 90
```

It disables direct IP transports, resolves by endpoint ID through the custom pkarr
service, transfers and hashes 4 MiB twice, with a 90-second idle period in between.
This proves forced relay transport; two endpoints on one machine do not establish
physical-device or cellular-network interoperability.

## Storage and operational limits

Discovery uses one SQLite Durable Object per allowed public key. Ed25519 signatures
and the DNS packet are verified before storage. Older timestamps and conflicting
records at the same timestamp cannot overwrite newer data, even with concurrent
requests. Packets are limited to 1072 bytes and timestamps to the last 24 hours
(with 5 minutes of future clock tolerance). Expired records return 404 while their
timestamps remain as replay protection. GETs are not edge-cached.

This service implements pkarr HTTP publishing/resolution, not authoritative DNS or
DHT discovery. UDP address discovery is disabled on this container; iroh direct
peer connectivity remains independent. The single relay is an initial deployment,
not a redundant multi-region service. Metrics are available inside the container
on port 9090 and are not exposed by the public Worker.

Use `wrangler tail --config wrangler.local.json`, Cloudflare container logs, and
the native probe together. A successful `/healthz` is not proof of relay availability.
Keep the previous Worker version/image and device network configuration for rollback.
