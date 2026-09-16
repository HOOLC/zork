# Development

- Preserve existing uncommitted work. Do not reset unrelated changes.
- Use the pnpm version declared in `package.json` (currently 10.33.0) and frozen lockfiles. Use `--locked` for Cargo builds and tests.
- Follow `docs/guides/rust-builds.md` when changing shared Rust dependencies or features.
- Rebuild affected binaries before process tests. Read current package scripts and workflows when selecting checks; historical validation reports are not proof for the current working tree.
- For code delivery, merge and release validation, follow [zork-critical-gates](.agents/skills/zork-critical-gates/SKILL.md) and run the user-approved smoke suite. Only explicit user approval changes its requirements; report a newly added test and product compliance separately.

# Skill maintenance

- Before reporting any requirement complete, follow [zork-skill-maintenance](.agents/skills/zork-skill-maintenance/SKILL.md) to check the relevant project skills against the final implementation and design. Update stale or missing guidance in the same task when warranted, and briefly report the result. The check is required for every requirement; edits require a concrete reason.

# Client core and UI

- UI is limited to presentation, animation, input capture and transient interaction state. All client business rules, validation, business state, network requests, persistence, synchronization, retries and operation lifecycles must go through `zork-client-core`.
- UI submits business intents and consumes read-only core snapshots or deltas. It must not construct HTTP methods/paths/bodies, implement business polling, infer delivery/authorization state, or maintain a second mutable business list. Moving a raw request behind a forwarding helper does not satisfy this boundary.
- Platform adapters provide capabilities requested through core-defined interfaces; they do not own business decisions. Core must remain independent of GPUI, Compose and widget lifetimes. Desktop, Android and Web fixtures use the same business contracts.
- Before implementing, refactoring or reviewing client features or core/UI separation, read [zork-client-boundary](.agents/skills/zork-client-boundary/SKILL.md) and follow [the core/UI boundary](docs/design/client-core.md). Existing violations are migration work, not exemptions for new code; inspect current call sites rather than maintaining a migration ledger.
- For core-to-UI subscriptions, snapshots/deltas, or high-frequency update delivery, read [zork-client-subscriptions](.agents/skills/zork-client-subscriptions/SKILL.md). Preserve consumer-applied version baselines, bounded recovery and platform scheduling boundaries; report validation against the tested revision in the task/PR or local artifacts, not in a permanent implementation-status document.

# Chat messages

- Before designing, implementing or reviewing Chat/channel behavior, delivered-message caching or interactive message cards, read [zork-chat-messages](.agents/skills/zork-chat-messages/SKILL.md). Keep the authoritative message source append-only while allowing Rust core to update merged client cache records; preserve result-before-request handling and source-based synchronization cursors.

# Local environment instructions

Before source changes, dependency installation, builds, tests, or starting development services, look for a `zork-local-environment` skill under `.agents/skills/` (including grouping subdirectories). If present, read its `SKILL.md` and apply its host and checkout rules before project work. If absent, use the current checkout and the repository's general instructions.

Personal environment skills belong in a locally excluded group, configured through `.git/info/exclude`. Shared skills must not require a contributor's private hosts, usernames, absolute checkout paths, or service endpoints.

<a id="repository-content"></a>

# Repository content

- Keep source, tests, lockfiles, required assets, original licenses/provenance and fixed comparison inputs. Credentials, databases, device state, build caches and raw run output stay in ignored local directories. Fixed benchmark corpora retain their bytes and hashes.
- Organize documentation by purpose first: enduring design constraints, unresolved proposals and operating guides. Name topics within each category by their responsibility, not by the current crate or UI location. Separate mixed content instead of merging unrelated topics to meet a file count.
- Each document needs an independent, ongoing reader purpose: a durable constraint, decision, compatibility promise or necessary operating guide. Useful paragraphs alone do not justify another file; merge rules with the same responsibility and remove duplication, rather than concatenating old documents.
- Source, generated help and tests own interface details. Do not copy module/field/route inventories, defaults, implementation status or test counts into permanent docs or skills. Record progress and results with the task/PR or tested artifacts. Completed proposals retain only useful decisions.
- Maintain one authority and the [documentation index](docs/README.md). Check unique content, readers, links, build consumers and design aliases before removing a document; preserve original decisions and functional inputs where needed. Do not create a document for every feature.
- Before publishing, inspect the exact candidate tree including untracked inputs, verify build completeness with frozen dependencies, and keep private data out of shared metadata and Git history. A deletion does not erase earlier commits. Publication is distinct from release/installation validation.
