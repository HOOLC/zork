# Cloudflare relay control plane

The Worker admits Google-authenticated accounts to one official iroh relay
container. Mesh invitations establish member trust; account login does not grant
access to another member's files or tools. LAN, direct connections and signed pkarr
discovery remain independent of Google login. UDP discovery and hole-punching
policy are separate transport responsibilities.

A strongly consistent Durable Object owns each account's sessions and relay
connections. Access credentials expire; refresh credentials rotate and have both
idle and absolute expiry. Retrying a lost refresh response uses the same persisted
request ID. Reusing an older credential outside that retry revokes its session.
Session revocation and account blocking close existing WebSockets, and byte, frame and
connection limits apply across all sessions of an account. Enforcement constants
and request contracts live in [the implementation](src/account.ts).

The browser returns a one-use code to a loopback listener, bound to client state
and PKCE. Google tokens, access tokens and refresh tokens are not placed in that
callback URL. Google identity verification uses its public keys and checks issuer,
audience, nonce, expiry and verified email. Invocation logging is disabled because
OAuth requests contain one-use codes; do not enable full URL/header capture.

## Configure

Use the pnpm version declared in package.json and frozen dependencies:

    cd deploy/cloudflare
    pnpm install --ignore-workspace --frozen-lockfile

In Google Auth Platform, create a **Web application** OAuth client. Configure the
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
silently omit the new session storage. Google client ID comes from the private
Google JSON. Its registered redirect must match the configured origin.

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

Verify real Google consent, native relay protocol traffic, automatic renewal,
relogin without Station restart, server revocation and reconnect denial. A healthy
/healthz only proves Worker reachability. Local mock Google identity tests do not
prove the real OAuth client's configuration or consent screen.

Desktop and Android expose Zork account login before device connection and in
settings. The desktop profile owns one private session shared by its embedded
client and owned Station, including after background service takeover. An
independent Station keeps its own account. For a headless host, run:

    zork account login --device --no-browser --data /path/to/station-data

Open the printed authorization link on any browser, confirm the requesting
device, and select a Google account. The requesting core polls with its private
PKCE verifier; no loopback tunnel or pre-existing Mesh connection is needed.
Use the same --data as the Station being tested. Do not copy account files or Mesh
identities between development and release profiles. CLI help describes session
revocation and account-wide logout. When offline, logout disables local access,
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

The Android runner requires an isolated emulator. It installs a fixed APK copy,
uses a loopback reverse forward and checks the real settings UI through JNI.
These local fixtures validate the product flow; real Google consent must also
be checked against the deployed OAuth client.

For Google-independent LAN bootstrap, run the isolated Station regression from
the repository root after rebuilding Station:

    python3 scripts/android/test_enrollment.py --test relay_account_enrollment

The native regression uses the production CLI, Station, credential controller and
official relay container. Only Google's external identity provider is a local
RS256 fixture. It waits through the production renewal interval. Test entrypoints
are separate from the deploy bundle.

Keep the previous Worker version and image, the deployment metadata saved by the
helper, and the private signing key for rollback. Durable Object migrations are
forward-only: retain the account classes/bindings and storage when rolling back
application logic. Do not roll back to the old stateless JWT admission path, which
cannot honor revoked sessions or enforce connected-account limits. Key rotation
invalidates future admission but is not a substitute for targeted session/account
revocation of existing sockets. Never restore a stale account database to undo a
revocation.
