---
name: skill-management
description: Create, improve or troubleshoot reusable Skill files and their discovery in Zork.
---

Skills are ordinary directories containing SKILL.md and optional resources. The runtime supplies the current catalog before model requests. Select by description and full path, then read with file.read; same-name candidates retain their own identity.

Write Skills for reusable best practices: decision criteria, trade-offs, task organization, quality checks and domain-specific pitfalls. Examples should explain a judgment, not reproduce an API reference.

Tool manuals belong in tool.help: invocation syntax, parameters, return values, identifier meanings, constraints, errors and their handling, and required procedures such as pagination or file uploads. A multi-step API procedure is still a manual, not a Skill. Refer to tool.help instead of copying those details into the body or references; missing help should be fixed at the tool, not supplied by a Skill.

Choose ordinary Skill sources for reusable guidance and keep device-specific or Agent-specific instructions scoped to their intended source. Use existing file tools and Agent configuration; consult their tool.help for current usage rather than creating separate Skill file-operation tools.

A manifest starts with --- frontmatter containing a short name and a description explaining when to use it, followed by useful instructions. Preserve resources and concurrent edits. Read the full file before editing. Remove or move an entire directory when retiring it so nested examples do not become separate Skills. Verify the resulting files; discovery refreshes before the next model request.

Release-managed files are read-only. To customize one, copy its complete directory, including resources, to an ordinary source and edit the copy. Software updates preserve user copies and each bundled Skill’s enabled state and version history.
