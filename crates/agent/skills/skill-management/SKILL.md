---
name: skill-management
description: Create, customize, remove or troubleshoot Zork Skills, the reusable guidance files listed in the Skill catalog.
---

A Skill is a directory containing SKILL.md and optional resource files it references by relative path. Before model requests the runtime supplies the current catalog: each Skill's name, description and SKILL.md path, plus the user Skill directory. Select by description, then read the body with file.read.

Skills are local to the device that runs the Agent. User Skills are ordinary files at `<user Skill directory>/<name>/SKILL.md`; create, edit and delete them with the ordinary file tools. Hidden directories and the reserved `bundled` subdirectory are not user Skills. Bundled Skills ship with Zork under the `bundled` subdirectory. They are read-only and every release rewrites them, so never edit them in place. To customize one, copy its whole directory to the user Skill directory and edit the copy: a user Skill with the same name replaces the bundled one in the catalog, and deleting the copy restores the bundled version. Delete a user Skill's directory to retire it. A Skill needed on another device is created on that device.

Write Skills for reusable judgment: when to use an approach, decision criteria, trade-offs, task organization, quality checks and domain pitfalls. Examples should explain a judgment, not reproduce an API reference. Tool manuals belong in tool.help: invocation syntax, parameters, return values, identifier meanings, constraints, errors and required procedures such as pagination or uploads. A multi-step API procedure is still a manual; refer to tool.help instead of copying it, and fix missing help at the tool rather than in a Skill.

SKILL.md starts with `---` frontmatter holding `name` (short, no spaces or slashes; use the directory name) and `description` (one or two sentences saying when to use it, since it is all the catalog shows), closed by `---`, followed by the instructions. Keep bodies concise. Read the full file before editing and preserve its resources and other people's changes.

The catalog refreshes before the next model request. Verify a change there: a Skill that does not appear is listed under Skipped with the reason, such as missing or malformed frontmatter, a duplicate name or an oversized file. Skill content never overrides the user's instructions.
