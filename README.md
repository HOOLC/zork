<p align="center">
  <img src="crates/zork-ui/assets/app/icon.png" alt="Zork icon" width="112" />
</p>

<h1 align="center">Zork</h1>

<p align="center"><strong>A local-first agent mesh.</strong></p>

<p align="center">
  <a href="#get-started">Get started</a> ·
  <a href="#the-mesh">The mesh</a> ·
  <a href="#documentation">Documentation</a> ·
  <a href="https://github.com/HOOLC/zork/actions">Build status</a>
</p>

Run persistent agents on your own devices, connect them through a mesh, and let
them collaborate. Agents keep durable state and work in local workspaces. They
communicate through chats and can delegate work to other agents and devices.

Use native desktop and Android clients to talk to your agents and manage the mesh.
Model requests go to the providers you configure.

![Hand-drawn Zork architecture: four peer Stations each host agents, skills, MCP connections, services, files and local state. Mesh links carry messages, delegation and authorized resource sharing. Desktop and Android clients connect to one or more Stations; a client and Station can run on the same device.](docs/images/agent-mesh.png)

## What you can do

- **Build an agent mesh.** Connect stations and clients with invitations and
  explicit permissions. Let agents communicate and delegate across devices.
- **Share capabilities across devices.** Call another Station's
  MCP tools, access shared services and exchange files through the mesh.
- **Work through conversations.** Choose agents and models, send messages and
  files, follow progress, and manage work from a native client.
- **Keep agents running.** Each agent session has a durable mailbox and event
  history. A Station can run in the background independently of the client.
- **Use your existing workspaces.** Agents run tools in an explicitly selected
  directory. They can read and edit files, run shell commands and use configured
  MCP services and skills.
- **Bring your own model connections.** Profiles manage provider credentials and
  model access. Slack is optional; the native conversation entry works without it.

## Get started

Zork is under active development. Start from source today; prebuilt node packages
are available only when a complete release is published on the
[Releases page](https://github.com/HOOLC/zork/releases).

You need Rust, Node.js 22.15 or later and pnpm 10.33.0. Coding tools also use
`git`, `gh` and `rg` from `PATH`. Native desktop builds require the platform
toolchain described in the [desktop README](crates/zork-gui/README.md).

```sh
git clone https://github.com/HOOLC/zork.git
cd zork
pnpm install --frozen-lockfile
pnpm build
pnpm dev
```

`pnpm dev` starts the supervisor and Station with the default data directory
`~/.zork`. For an isolated node:

```sh
pnpm dev -- --data /absolute/path/to/node-data
```

On macOS, start the desktop client from the same checkout:

```sh
python3 scripts/lib/build_env.py -- cargo run --locked -p zork-gui
```

Configure model connections and agents in Settings, then select an agent from the
conversation sidebar. The client can manage its local node or connect to other
devices. Product settings live in the node's `config.json`; local build settings
may use an ignored `.env`. See [desktop setup](docs/design/devices.md#lifecycle) and
[Android development](apps/android/README.md).

<details>
<summary>Install a published native node</summary>

Once a complete native release is available:

```sh
curl -fsSL https://github.com/HOOLC/zork/releases/latest/download/install.sh | sh
```

The installer selects and verifies a native package and starts a persistent
Station. Node.js and npm are not required on an installed node. To join a mesh,
use the pinned invitation command from Settings → Device connections.
[Native release instructions](docs/guides/native-releases.md) cover offline installs,
version pinning, service management and upgrades.

`zork update` restarts an existing node with already staged binaries.
`zork upgrade --version X.Y.Z` downloads and activates a complete release.
Neither a fresh install nor joining a mesh silently upgrades a running node.

</details>

## The mesh

- **An agent** runs a persistent session with a mailbox, tools and a local
  workspace. It can exchange messages and delegate tasks to other agents.
- **A Station** hosts agents on one device. It owns local state, conversations,
  skills, MCP connections, services and files, and participates in the mesh as a peer.
- **A client** connects to Stations to start conversations, configure agents and
  manage devices and resources. One client can connect to multiple Stations;
  a client and Station can also run on the same device. Stations can keep working
  after the client closes.

The mesh carries both agent collaboration and access to shared capabilities.
Each owning Station checks the permissions it has granted:

| Resource     | Across the mesh                                                                   |
| ------------ | --------------------------------------------------------------------------------- |
| **Skills**   | Local to each Station: bundled guidance plus user Skills; not shared.             |
| **MCP**      | Discover and call tools on the Station that hosts the connection and credentials. |
| **Services** | Open explicitly shared applications through their hosting Station.                |
| **Files**    | Exchange selected attachments and artifacts; receivers keep their own snapshots.  |

Each Station keeps its own state and workspace. Sharing is explicit; joining the
mesh does not automatically replicate entire directories. See
[device tools](docs/design/external-capabilities.md), [Agent Skills](docs/design/agent-skills.md), [Mesh MCP](docs/design/external-capabilities.md#mcp),
[service sharing](docs/design/external-capabilities.md#services) and [file sharing](docs/design/shared-files.md#attachments).

## Development

The `zork` supervisor starts one `zork-station` process, which embeds the agent
runtime. The standalone `zork-agent` binary is also available. Clients submit
business intents through `zork-client-core` and render its state. See the
[agent architecture](docs/design/agent-runtime.md),
[client boundary](docs/design/client-core.md) and
[chat contracts](docs/design/chat.md).

This repository includes a macOS desktop client and Android client. Native node
packaging targets macOS and Linux on ARM64 and x64; desktop app packaging and
signing follow a separate workflow. The native design app is an interactive
reference and component showcase. See [native releases](docs/guides/native-releases.md)
for platform requirements.

<details>
<summary>Repository map and validation commands</summary>

### Repository map

| Path                                                       | Responsibility                                                |
| ---------------------------------------------------------- | ------------------------------------------------------------- |
| `crates/station`                                           | Station: conversations, delivery, node APIs and orchestration |
| `crates/agent`, `crates/agent-http`, `crates/agent-server` | Durable agent runtime and optional HTTP host                  |
| `crates/zork`                                              | Supervisor, installation, services and upgrades               |
| `crates/zork-client-core`, `crates/zork-client-types`      | Shared client operations, state, sync and contracts           |
| `crates/zork-gui`, `crates/zork-ui`                        | Desktop application and reusable GPUI components              |
| `apps/android`, `crates/zork-android`                      | Android application and Rust bridge                           |
| `crates/zork-mesh`                                         | Device transport, enrollment and synchronization support      |
| `crates/profile`, `crates/slack`                           | Model profiles and Slack integration                          |
| `apps/zork-design-pc`, `zork-design-pc`                    | Editable design sources and native component browser          |
| `scripts`, `.github/workflows`                             | Development, packaging and validation                         |
| `benchmarks`                                               | Independent benchmarks and experiments                        |
| `docs`                                                     | Architecture, contracts and operating guides                  |

### Development checks

Install local checks with `pnpm hooks:install`. Commit hooks run fast static and
boundary checks; push hooks run the affected test suites. Branches and PRs do not
start automatic CI. Main runs the shared runtime contracts and path-scoped
Cloudflare validation; native packaging and upgrade checks remain in tag releases.

```sh
pnpm format:check
pnpm lint
pnpm build              # rebuild binaries before process tests
pnpm test               # JS and process contracts
pnpm test:rust          # backend Rust tests
bash scripts/check-runtime.sh # the same runtime suite used on main
pnpm test:desktop       # native desktop tests
python3 scripts/check-client-boundary.py
```

Native design browser: `pnpm design:pc` or `pnpm design:pc:verify`. DeepSWE adapter tests: `pnpm benchmark:deep-swe:test`.
Functional tests and performance measurements have separate entry points; see
[validation](.agents/skills/zork-validation/SKILL.md) and [repository content](AGENTS.md#repository-content).

</details>

## Documentation

The [documentation index](docs/README.md) groups Agent, Chat, client, Mesh, UI and
engineering contracts and pending proposals. Implementation details stay with the
source and generated help; test results belong to their run artifacts.
The [native design browser](apps/zork-design-pc/README.md) contains editable guidelines,
curated assets and historical visual references.

## License

[MIT](LICENSE). Bundled third-party components retain their own license and
attribution files alongside the source and assets.
