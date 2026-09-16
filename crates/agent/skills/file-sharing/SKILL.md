---
name: file-sharing
description: Choose where Zork task files belong, put files in the user's shared directory, or read and reuse shared files. Use for file deliverables and cross-device file access.
---

# File placement and sharing

The execution workspace is the current directory for shell commands and relative file paths. Read `REPOS_ROOT` and `SHARED_FILES_ROOT` from the current shell environment for the actual repository and user shared directories; do not infer paths from another device or an old transcript.

- The workspace is the default destination for all task files, including finished deliverables. A request to create a file and report its location does not request sharing. Keep canonical repository clones under `REPOS_ROOT`.
- When the user requests sharing, access from another device, or placement in the shared directory, put the selected files under `SHARED_FILES_ROOT`. Use ordinary local file operations to create, copy, move or edit them. Synch watches this directory and publishes changes automatically; there is no separate sharing operation. Choose file handling and replacement behavior according to the task, and preserve explicit user destinations.
- Chat attachments use `chat.send` with a file from the execution workspace; it freezes the selected file. Attaching a file does not require another copy into the shared directory.
- Skill sources and installation use `skill-management`; running websites and services use `service-sharing`.

When confirming shared visibility, browse `synch://shared/` with `file.list` and read the selected file with `file.read`. Synch publication is asynchronous; local storage and observed shared availability are different facts. A returned fixed reference preserves its `origin`, `root` and `snapshot`; start from a live directory reference when looking for later changes.

`synch://` references are read-only. Use `file.materialize` when a command needs local paths; its result is an immutable cache. Copy files into the workspace before editing them, then put the edited files in the shared directory if the task calls for it.
