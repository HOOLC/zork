# Cloudflare relay control plane

The Worker coordinates signed discovery and one official iroh relay container.
Device-held keys and local Mesh membership authorize native peer requests; the
account directory establishes same-account phone access; the cloud cannot decrypt end-to-end business traffic. Relay and
LAN/direct connections work without a Google account.

One Durable Object proxies relay WebSockets and bounds connections, upgrade
attempts, bytes and frames, including pending upgrades and empty frames. Budgets
are per client so one client cannot lock out everybody else:

| key | concurrent | connects/min | bytes/day | frames/day |
| --- | --- | --- | --- | --- |
| client IP (IPv6 by /64) | 32 | 60 | 4 GiB | 8M |
| verified EndpointId | 4 | 20 | 2 GiB | 4M |
| whole service | 256 | 600 | 20 GiB | 40M |

The EndpointId budget applies once the relay process has confirmed the client's
signed challenge; the unverifiable identity header on the upgrade is ignored.
Rejected upgrades get HTTP 429 with `Retry-After`. A used-up daily budget closes
only the sockets charged to that key (code 4008); a momentary rate burst closes
only the sending socket. Connections must complete the relay handshake within
30 seconds. The global cap is sized to Cloudflare cost (see the comment in
[the relay implementation](src/relay.ts)); it protects operating cost and does
not guarantee availability against an attacker using many addresses. No
business RPC or additional bridge protocol passes through the Worker: it
forwards native iroh relay frames.

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
Objects and Containers, and the target zone's Worker routes/custom domain.
Cloudflare Containers must be enabled and Docker must build Linux amd64 images.
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
Worker, the pinned relay image archive, public deployment configuration and
SHA-256 manifest. It performs a Wrangler dry run and an amd64 container build.
It rejects an existing output directory to preserve a reviewed candidate.

## Deploy and accept

After filling configuration gaps, prepare a **new** candidate. Deploy exactly that
candidate:

    python3 deploy.py deploy --config wrangler.local.json --output /path/to/candidate

The helper checks artifact hashes and configuration, loads the fixed image, stores
Worker secrets, and deploys. It generates independent random signing and admin
keys in ~/zork-deploy/session-keys.json on first use. Retain that private file for
subsequent deploys; it is not part of the candidate. A temporary Docker config
avoids an SSH session's locked macOS keychain without modifying global settings.
Secret files are never embedded in the Worker or passed as command-line values.

A deployment command succeeding is not acceptance. Use a clean, isolated data
directory and the freshly built CLI/Station against the deployed origin:

    zork account login --data /path/to/isolated-data
    zork account status --data /path/to/isolated-data --json
    zork account refresh --data /path/to/isolated-data
    zork account sessions --data /path/to/isolated-data
    zork account logout --data /path/to/isolated-data

For the first upgrade from stateless admission, add `--restart-relay` to the
deploy command. After deployment it uses the private admin credential to retire
the old relay process and its untracked sockets. Verify that pre-upgrade WebSockets
have closed and new connections use the deployed image. An immediate Cloudflare
rollout starts replacement; it does not prove the old process has exited. The
restart preserves account and discovery storage. Ordinary compatible redeploys
can let the rollout drain existing connections.

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

The relay process accepts only the Worker's internal forwarding and has no public
container endpoint. All admitted peers reach the same container; independently
routing upgrades would split the relay. The container's metrics port is internal.
Signed discovery stores bounded, verified packets; it is neither an account
directory nor a device allowlist.

## Local regression and rollback

    pnpm types
    pnpm check
    pnpm test
    # After rebuilding zork and zork-station with --locked:
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

The native regression uses the production CLI, Station and official relay
container without Google configuration. Account UI tests use a local RS256 Google
fixture. Test entrypoints are separate from the deploy bundle.

Keep the previous Worker version and image, the deployment metadata saved by the
helper, and the private signing key for rollback. Durable Object migrations are
forward-only: retain the account classes/bindings and storage when rolling back
application logic. Retain relay budget storage across compatible updates so a
restart cannot reset consumed quotas. Never restore a stale account database to
undo a revocation. Rolling back to account-gated relay admission changes the
transport contract and is not an interchangeable rollback.
