# Native node distribution

GitHub Releases is the primary distribution channel for standalone Zork nodes.
Each version contains one complete archive per platform, `install.sh`, `VERSION`,
`manifest.tsv` and `SHA256SUMS`. Node.js is only a repository development tool;
it is not an installation prerequisite. The old npm launchers remain available
for compatibility work, but CI no longer publishes npm packages.

This pipeline distributes the supervisor, Station, Agent and GitHub helper.
Station owns iroh connections on its Tokio runtime. No file synchronization
engine or separate transport daemon is bundled. Desktop `.app` distribution remains separate in
`scripts/package-macos-client.py`; it requires its own release signing and
notarization setup before public desktop distribution.

## Supported environments

| Package      | Minimum supported environment |
| ------------ | ----------------------------- |
| darwin-arm64 | macOS 15, Apple Silicon       |
| darwin-x64   | macOS 15, Intel               |
| linux-arm64  | ARM64 glibc Linux, glibc 2.39 |
| linux-x64    | x64 glibc Linux, glibc 2.39   |

The Linux baseline follows the Ubuntu 24.04 build runners. Alpine/musl and older
glibc releases are rejected rather than attempting to run an incompatible
executable. This is separate from the higher minimum OS of the desktop GUI.
Installation requires a POSIX shell, curl, tar, awk and either sha256sum or shasum.
Background nodes use the logged-in macOS user's launchd domain or Linux user
systemd; a usable user service manager is required. Coding work also needs git,
gh and rg. A package manager is not invoked automatically to install prerequisites.

## Install and join

These public URLs become usable when the corresponding native release exists:

```sh
# Latest stable release: install/start an independent background node.
curl -fsSL https://github.com/HOOLC/zork/releases/latest/download/install.sh | sh

# Pin the bootstrap and package version; forward native arguments after --.
curl -fsSL https://github.com/HOOLC/zork/releases/download/v0.1.30/install.sh \
  | sh -s -- --version 0.1.30 -- install --data "$HOME/.zork" --name mini2
```

The device connection UI generates an installation page link. Its invitation
stays in the URL fragment; the page server and GitHub never receive it. The page
checks that the published release includes every platform and integrity input
before displaying a pinned bootstrap command. Without a complete release it
explains that installation is unavailable instead of inventing a download URL.
The installer forwards the invitation to the native join command and preserves
existing node identity and tasks.

Publish the matching node packages before enabling the installation page for a
new transport version. Endpoints must use the current transport protocol and identity store; Synch
identity and attachment migration are not supported. Desktop application distribution and account Worker deployment
are separate from the native node release.

The default install starts a node without creating a mesh invitation. Configure
model connections and Agents through the desktop client afterward. The native
command prints its data directory; the installed CLI is `<data>/bin/zork`.
PATH and shell startup files are not modified.

The installer supports another trusted source through `--base-url` or
`ZORK_RELEASE_BASE_URL`. The base must use HTTPS and preserve these paths:

```text
latest/download/VERSION
download/v0.1.30/manifest.tsv
download/v0.1.30/zork-0.1.30-darwin-arm64.tar.gz
```

For offline installation, download the script, manifest and matching archive
from the same release. Arrange the manifest and archive in
`/path/to/releases/download/v0.1.30/`, then run:

```sh
sh /path/to/install.sh --base-url file:///path/to/releases --version 0.1.30 -- install
```

The manifest contains tab-separated platform, archive filename, SHA-256 and
minimum OS/libc version fields. The installer validates its selected entry,
checks the downloaded archive before extracting, rejects unexpected members,
links and special files, and verifies the bundled version and executable set.
The manifest's trust comes from the selected HTTPS release source (or the
operator's offline copy); SHA-256 alone is not an independent publisher signature.

## LAN test packages

Test installation packages use a separately configured S3-compatible store such
as MinIO. Build caches are independent. Keep the endpoint, bucket and upload
credentials in private local configuration, never in the repository or app.
`scripts/build/publish-test.py --help` describes the publisher inputs. It verifies
native stage outputs, includes explicitly selected APK/desktop archives, uploads
a content-addressed candidate, then writes credential-free distribution metadata.
A test candidate may contain only the platforms actually built; its manifest must
not claim unsupported platforms.

Pass that metadata to `scripts/package-macos-client.py --channel test
--test-distribution /path/to/test-distribution.json`. A device-local
`test-distribution.json` or `ZORK_TEST_DISTRIBUTION` file can override the packaged
source. Missing test metadata is an error; test never falls back to public
releases. The invitation page keeps the pinned source and invitation in its URL
fragment. HTTP transport is allowed only with the installer's explicit
`--allow-http-test` flag and native `--channel test`, for a trusted development LAN.
Public releases still require HTTPS. Download checksums detect damaged packages;
they do not authenticate an HTTP server.

## Manage an installed node

Use the pinned invitation command generated by the desktop client to add a Station;
phones connect after signing in to the same Google account. After joining, configure
models and Agents on that target device. Multiple local instances require an
explicit data directory.

```sh
zork mesh join '<invitation>' --data /absolute/path/to/node
zork mesh status --data /absolute/path/to/node
zork service install --data /absolute/path/to/node
zork service status --data /absolute/path/to/node
zork service uninstall --data /absolute/path/to/node
zork stop --data /absolute/path/to/node
```

Removing the service registration does not stop an existing node. Background and
client-owned lifecycle rules are defined in [device ownership](../design/devices.md#lifecycle).
Use `zork service --help` for options; do not copy another device's identity or data
to create a new installation.

## Build and publish

Use the version in root `package.json` as the native release version. Existing
npm metadata currently shares that version for legacy compatibility. All Zork
components in an archive come from one source checkout; transport dependencies use the versions pinned in `Cargo.lock`.

```sh
CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_BUILD_JOBS=4 \
  cargo build --locked --release -p zork -p zork-station -p zork-agent-server -p zork-gh
python3 scripts/build/native-release.py stage
python3 scripts/test-native-installer.py
python3 scripts/test-mesh-installer.py  # real user service test on macOS
```

`stage` only reads `target/release`; it never falls back to debug output or
downloads a separate transport executable.
Only the current platform is staged locally. `assemble` requires all four
platform archives and metadata, verifies every component list, bundled version
and archive digest, and generates the shared release assets.

`.github/workflows/release.yml` builds and tests four native platforms. Pull
requests and manual branch runs produce reviewable workflow artifacts. A `v*`
tag matching the package version enables the publish job: create a draft,
upload the complete verified asset set, then publish. Stable tags become latest;
prerelease tags do not. A failed upload leaves a draft for retry; published
versions are never overwritten. GitHub's per-job token handles publishing;
no npm token or additional storage service is needed.

## Updates and compatibility

Installation and mesh joining do not perform in-place upgrades. For complete
installed background nodes, open the desktop device settings, choose **检查更新**,
then **升级并重启**. The Station administrator credential is required. The client
shows the download, restart, and completion state; the node continues the upgrade
if the client disconnects. App-bundled and development nodes must be updated through
their application or development deployment instead.

The node downloads the selected stable release from the fixed GitHub Releases
source, using the bundled bootstrap's manifest, SHA-256, platform and archive
validation. All components are staged together before any running process changes.
The target supervisor must advertise `native_update: 1`. The supervisor asks the
Station and its embedded Agent to stop, waits up to 30 seconds, renames the complete `bin` directory,
and executes the new supervisor with the original arguments and PID. Upgrading
interrupts running tasks; choose a suitable time. No forced kill is used if a child
cannot stop within the deadline.

Previous binaries remain in `run/update-previous-*`. If launching the new executable
fails before it starts, the old directory is restored. Once new code has run,
data is never rolled back automatically; a failed health check is reported for
operator diagnosis. Preserve compatible state backups when planning a release
that changes data formats. Upgrade logs are in `logs/update.log`, and the latest
operation state is in `run/update.json`.

The same operation is available as `zork upgrade --version X.Y.Z --data DIR`.
`zork update` keeps its existing drain-and-restart meaning for already staged
binaries. Neither installation nor mesh joining silently upgrades a node.
