---
name: file-sharing
description: Choose where Zork task files belong and how to deliver files to the user or another device. Use for file deliverables and cross-device file access.
---

# File placement and delivery

The execution workspace is the current directory for shell commands and relative file paths; the shell environment names it `SESSION_WORKSPACE`. Canonical repository clones belong under `REPOS_ROOT`. Read both from the current environment; do not infer paths from another device or an old transcript.

- The workspace is the default destination for task files, including finished deliverables. Creating a file and reporting its location does not request delivery. Preserve destinations the user names explicitly, and do not overwrite existing user files as a side effect.
- Send a file with `chat.post_file` when the user wants to open, download or keep it, or use it on another device. Any readable file on this node can be sent without copying it into the workspace first. The attachment is a fixed snapshot: later edits need another delivery. Say in the message what the file is and what changed, so the reader need not open it to find out. Bundle many related files into one archive when the reader needs them together.
- Zork has no shared or synchronized directory. Placing a file somewhere does not make it appear on another device. Across devices, attach the file to the relevant Chat, or work on the device that owns it through the tools' device target.
- Running websites and services use `service-sharing`; reusable guidance files use `skill-management`.
