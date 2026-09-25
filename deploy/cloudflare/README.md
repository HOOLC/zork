# Cloudflare relay control plane

The Worker coordinates signed discovery and runs the public iroh relay itself.
Device-held keys and local Mesh membership authorize native peer requests; the
account directory establishes same-account phone access; the cloud cannot decrypt end-to-end business traffic. Relay and
LAN/direct connections work without a Google account.

## Relay

One Durable Object (`RelayHub`, [src/relay.ts](src/relay.ts)) speaks the iroh
relay protocol natively ([src/relay-protocol.ts](src/relay-protocol.ts)),
wire-compatible with iroh-relay 1.1 clients (`iroh-relay-v2` and `-v1`
subprotocols): signed challenge (`ServerChallenge`/`ClientAuth`, ed25519 over
BLAKE3 `derive_key`), `ServerConfirmsAuth`/`ServerDeniesAuth`, datagram and
datagram-batch forwarding by EndpointId (ECN and segment size preserved),
Ping/Pong, `EndpointGone` to peers a departed endpoint sent to, `Status`/`Health`
notices, advisory `Restarting` on an operator restart, and iroh-relay's rule for
duplicate EndpointIds: the newest connection receives traffic, the displaced one
stays open, is told `SameEndpointIdConnected`, and is promoted back (`Healthy`)
when the newer one closes. Datagrams for an endpoint that is not connected are
dropped, as in iroh-relay.

The TLS keying-material header (`x-iroh-relay-client-auth-v1`) cannot be verified
behind Cloudflare, so every client proves its key with the challenge (one extra
round trip, iroh's built-in fallback). The header's EndpointId is only used as a
routing tag and must match the proven key.

The relay uses the WebSocket Hibernation API. Socket tags (client IP, claimed
EndpointId) and attachments (protocol version, verified EndpointId, connection
order, peers sent to) survive eviction; in-memory maps are caches. The relay does
not ping clients (iroh clients ping every 15 s and it answers), so an idle
object can be evicted. A socket that sends nothing for 5 minutes, or does not
authenticate within 30 seconds, is closed by the periodic alarm.

Bounds apply to connections, upgrade attempts, bytes and inbound messages,
including pending upgrades and empty frames. Budgets are per client so one
client cannot lock out everybody else:

| key                     | concurrent | connects/min | bytes/day | frames/day |
| ----------------------- | ---------- | ------------ | --------- | ---------- |
| client IP (IPv6 by /64) | 32         | 60           | 4 GiB     | 8M         |
| verified EndpointId     | 4          | 20           | 2 GiB     | 4M         |
| whole service           | 512        | 600          | 20 GiB    | 40M        |

The EndpointId budget applies once the client's signature over the challenge
verified; a denied handshake never charges the claimed identity. Rejected
upgrades get HTTP 429 with `Retry-After`. A used-up daily budget closes only the
sockets charged to that key (code 4008); a momentary rate burst drops messages
(like a congested path) and sends `Status(RateLimited)` once. The global cap is
sized to Cloudflare cost (see the comment in
[the relay implementation](src/relay.ts)); it protects operating cost and does
not guarantee availability against an attacker using many addresses. No
business RPC passes through the Worker: it relays end-to-end encrypted QUIC
datagrams between iroh endpoints.

Counters live in memory and are written to SQLite only when a key moved by at
least 1 MiB or 1000 messages (within 5 seconds, before the object can hibernate)
or at the 5-minute sweep, so an eviction forgets less than that per key.

### Cost

With hibernation, duration is ≈ 0 while the relay is idle; it is billed for
inbound WebSocket messages (20 messages = 1 request, outbound messages are free),
Worker requests for upgrades and latency probes, and sparse SQLite writes. There
is no container. Traffic measured before this change (~1.5M inbound messages and
~190 MB per week) is ≈ 75k billed requests per week. While messages arrive more
often than every ~10 seconds (several online clients each pinging every 15 s)
the object stays resident: at most one 128 MB object, within the Workers Paid
included duration. Workers do not bill egress.

Clients list n0's public relays as fallbacks after relay.zork.ing
(`fallback_relay_urls` in crates/zork-config/src/services.default.json). They
are in iroh's relay map but become the home relay only while relay.zork.ing does
not answer latency probes (vendored iroh patch, see vendor/iroh/ZORK-PATCH.md).

Optional cloud accounts have separate sessions. Access credentials expire;
refresh credentials rotate with idle and absolute expiry. Retrying a lost refresh
response uses the persisted request ID; reuse outside that retry revokes the
session. Logout removes account-owned device access; independently installed
members and their direct/relay connectivity remain available.
Account contracts live in [the account implementation](src/account.ts).

The browser returns a one-use code to a loopback listener, bound to client state
and PKCE. Google tokens, access tokens and refresh tokens are not placed in that
callback URL. Google identity verification uses its public keys and checks issuer,
audience, nonce, expiry and verified email. Invocation logging is disabled because
OAuth requests contain one-use codes; do not enable full URL/header capture.

## Configure

Use the pnpm version declared in package.json and frozen dependencies:

    cd deploy/cloudflare
    pnpm install --ignore-workspace --frozen-lockfile

For optional cloud accounts, in Google Auth Platform create a **Web application** OAuth client. Configure the
consent screen for openid and email; add the intended account as a test user
while the application is in testing. Register this exact callback, replacing the
host for a different deployment:

    https://relay.zork.ing/v1/auth/google/callback

Save the downloaded Web client JSON outside the checkout. The default location is
~/zork-deploy/google-oauth.json. For Cloudflare, run `pnpm exec wrangler login` and approve the browser prompt.
Use `pnpm exec wrangler login --device` if the browser is on another machine.
Alternatively, save a Cloudflare API token in ~/zork-deploy/cloudflare-api-token. Both must have mode 600.
The deployment identity needs access to the target account's Workers, Durable
Objects, and the target zone's Worker routes/custom domain.
See [Cloudflare API token setup](https://developers.cloudflare.com/fundamentals/api/get-started/create-token/)
and [Google OpenID Connect](https://developers.google.com/identity/openid-connect/openid-connect).

The canonical service is `https://relay.zork.ing`; its custom-domain route also
serves account endpoints and signed discovery. Wrangler creates the custom-domain
DNS mapping and certificate in the account containing the active `zork.ing` zone.
When migrating from a different issuer origin, sign in again; credentials are never
forwarded across origins. Register the new Google callback before cutting over.

Copy wrangler.jsonc to the ignored wrangler.local.json. Set account_id, the Worker
name, vars.PUBLIC_ORIGIN and its custom-domain route. The helper always takes code,
bindings and migrations from the checked-in template; an old private config cannot
silently omit the new session storage. When configured, the Google client ID comes from the private
Google JSON and its registered redirect must match the configured origin. With no
Google client ID, relay and discovery can be deployed without Google credentials.

    python3 deploy.py check --config wrangler.local.json
    python3 deploy.py prepare --config wrangler.local.json --output /path/to/candidate

The check reports missing inputs without their values. Preparation can produce a
reviewable candidate with configuration gaps recorded: a bundled production
Worker, public deployment configuration and SHA-256 manifest. It performs a
Wrangler dry run.
It rejects an existing output directory to preserve a reviewed candidate.

## Deploy and accept

After filling configuration gaps, prepare a **new** candidate. Deploy exactly that
candidate:

    python3 deploy.py deploy --config wrangler.local.json --output /path/to/candidate

The helper checks artifact hashes and configuration, stores Worker secrets, and
deploys. It generates independent random signing and admin
keys in ~/zork-deploy/session-keys.json on first use. Retain that private file for
subsequent deploys; it is not part of the candidate. Secret files are never embedded in the Worker or passed as command-line values.

A deployment command succeeding is not acceptance. Use a clean, isolated data
directory and the freshly built CLI/Station against the deployed origin:

    zork account login --data /path/to/isolated-data
    zork account status --data /path/to/isolated-data --json
    zork account refresh --data /path/to/isolated-data
    zork account sessions --data /path/to/isolated-data
    zork account logout --data /path/to/isolated-data

The first deploy of the native relay applies migration `v4-native-relay`: it
deletes the `Relay` container class and the `RelayBudget` proxy (and their
storage: relay budget counters only) and creates `RelayHub`. Every current relay
connection is closed once and reconnects to the new object; clients retry with
their backoff. Remove the old container application in the Cloudflare dashboard
if it remains listed. Account and discovery storage are untouched. Any Worker
deploy disconnects hibernated WebSockets once. `--restart-relay` (optional) uses
the private admin credential to send `Restarting` and close relay connections.

Verify native relay business traffic without account credentials and rejection
of unpaired peers by the receiving device. Verify optional Google consent,
renewal and server session revocation separately; they must not interrupt Mesh. A healthy
/healthz only proves Worker reachability. Local mock Google identity tests do not
prove the real OAuth client's configuration or consent screen.

Desktop and Android expose Zork account login before device connection and in
settings. The desktop profile owns one private session shared by its embedded
client and owned Station, including after background service takeover. An
independent Station needs no account for Mesh. For optional cloud login on a headless host, run:

    zork account login --device --no-browser --data /path/to/station-data

Open the printed authorization link on any browser, confirm the requesting
device, and select a Google account. The requesting core polls with its private
PKCE verifier; no loopback tunnel or pre-existing Mesh connection is needed.
Use the same --data as the Station being tested. Do not copy account files or Mesh
identities between development and release profiles. CLI help describes session
revocation and account-wide logout. When offline, logout disables local account credentials,
retains a private revocation record and returns a pending result. The running
account controller or another logout attempt retries it; only server confirmation
means remote logout finished.

All relay upgrades reach the same object; independently routing upgrades would
split the relay.
Signed discovery stores bounded, verified packets; it is neither an account
directory nor a device allowlist.

## Local regression and rollback

    pnpm types
    pnpm check
    pnpm test
    # Real iroh endpoints (no direct UDP) through `wrangler dev`:
    cargo build --locked -p zork-mesh --example relay-interop
    pnpm exec wrangler dev --ip 127.0.0.1 --port 18787 --var PUBLIC_ORIGIN:http://127.0.0.1:18787 --local-upstream 127.0.0.1:18787
    target/debug/examples/relay-interop check http://127.0.0.1:18787
    ZORK_RELAY_INTEROP_URL=http://127.0.0.1:18787 cargo test --locked -p zork-mesh --lib enrollment_exchange_through_external_relay -- --ignored

For TLS like production (clients then send the auth header), create a throwaway
certificate with `relay-interop cert DIR`, run `wrangler dev` with
`--local-protocol https --https-key-path DIR/key.pem --https-cert-path DIR/cert.pem`
and set `RELAY_INTEROP_CERT=DIR/cert.der` for the check. # After rebuilding zork and zork-station with --locked:
pnpm exec tsx test/native-lifecycle.ts /path/to/fresh/binaries /path/to/report

Product entry-point checks use the same local Worker and signed Google fixture:

    cargo test --locked -p zork-gui --features headless-bench --test headless_relay_account --no-run
    pnpm exec tsx test/product-account.ts /path/to/headless_relay_account /path/to/fresh/binaries /path/to/report
    pnpm exec tsx test/android-account.ts /path/to/fresh.apk emulator-SERIAL /path/to/report
    pnpm exec tsx test/device-browser.ts /path/to/browser-report

The Android runner requires an isolated emulator. It installs a fixed APK copy,
uses a loopback reverse forward and checks the real settings UI through JNI.
These local fixtures validate the product flow; real Google consent must also
be checked against the deployed OAuth client.
The browser check submits the actual confirmation form in Chromium; an HTTP
302 alone does not verify Origin, cookies or browser navigation policy.

The native regression uses the production CLI, Station and the Worker's native
relay without Google configuration. Account UI tests use a local RS256 Google
fixture. Test entrypoints are separate from the deploy bundle.

Keep the previous Worker version, the deployment metadata saved by the
helper, and the private signing key for rollback. Durable Object migrations are
forward-only: retain the account classes/bindings and storage when rolling back
application logic. Retain relay budget storage across compatible updates so a
restart cannot reset consumed quotas. Never restore a stale account database to
undo a revocation. Rolling back to account-gated relay admission changes the
transport contract and is not an interchangeable rollback.
