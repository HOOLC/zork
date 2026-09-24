Connect a new computer to the user's Mesh.

`mesh.invite` mints a one-time invitation on the Mesh managing Station and returns `id`, `command`, `expires_at`, `expires_in_seconds`, `single_use: true` and, when available, `install_url`. Optional `label` is a short note such as the computer's name; it is kept with the invitation and shown by `mesh.invites`. The command is `zork mesh join '<ticket>' --channel <channel>` and needs the Zork CLI on the new computer; `install_url` installs Zork there and carries the same ticket.


The command and install_url are Mesh admission secrets: whoever runs them first joins a device with full trust. Treat them as secrets:
- Run the command yourself on the target computer (for example with shell.run on a Mesh device that can reach it), or give it to the requesting user in the conversation where they asked.
- Never post it anywhere else: other conversations, issues, pull requests, published logs, notes, or files in synced/shared folders.
- If it may have been exposed, or is no longer needed, revoke it with `mesh.revoke`.
It expires after about 15 minutes and works once. Calling `mesh.invite` again creates another invitation; the Station limits open invitations, so revoke unused ones. A repeated delivery of the same invocation returns the invitation without the command (`command_shown: false`).

`mesh.invites` lists invitations with `state`: `created` (usable; `joining: true` while a device is completing the join), `used` (with `device.id` and `device.name` of the joined device), `expired` or `revoked`, plus `requested_in_this_chat` and `label`. By default it shows open invitations and those requested in this Chat; `include_inactive: true` adds the rest. It never returns commands. After the join, confirm the device with `device.list`, or `zork mesh status` on a Station.

`mesh.revoke` takes the `id` of a `created` invitation and makes it unusable. A used invitation cannot be revoked; the device must be removed from the Mesh by the user in device settings.

Errors: `mesh_not_ready` (Mesh not running on this Station), `too_many_active_invites`, and an unreachable managing Station (invitations are issued by it) are reported with a readable `message`.
