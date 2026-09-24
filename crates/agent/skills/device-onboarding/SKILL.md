---
name: device-onboarding
description: Add a computer or phone to the user's Zork Mesh, follow up on a pending join, or troubleshoot a device that does not appear. Use when the user wants another device in the Mesh or asks about an invitation.
---

# Device onboarding

Phones need no invitation. The user installs the Zork app on the phone and signs in with the same Google account as their other devices; the phone then joins by itself. Do not create an invitation for a phone.

A computer joins with a one-time invitation. `mesh.invite` returns a short-lived join command; run once in a terminal on the new computer, it installs Zork there and joins the Mesh. Use tool.help for its arguments and result.

## The command is a secret

Whoever runs an unused command first joins the Mesh with full trust. Keep it between the requesting user's conversation and the new computer:

- Do not post it to other conversations, write it into shared, synchronized or committed files, or leave it in logs you publish.
- Get commands only from `mesh.invite`. Do not produce one through a shell (such as `zork mesh invite`), which bypasses these checks.
- Create one invitation per computer and reuse a pending one while it is valid. If a command may have been exposed, or is no longer needed, revoke it with `mesh.revoke`. Revoking does not remove a device that already joined.

## Running it

If you can reach the new computer yourself, for example the user names a host you can ssh to from a Mesh device, run the command there and follow its output. Confirm the target is the intended new computer: never run it on a device that is already a Mesh member, including the one you execute on.

Otherwise send the command to the requesting user in the conversation where they asked, with where to run it (a terminal on the new computer), that it works once and expires soon, and that it installs Zork. Do not ask the user to relay it through other channels.

## Confirming the join

`mesh.invites` shows each invitation's state: created (not yet used), used (with the device that joined), expired or revoked. A used invitation shows the join was accepted; confirm the new device in `device.list` before reporting success, and name the device.

- Expired before use: create a new invitation. An old command never becomes valid again.
- Still created after the user ran it: the command did not reach the join step; read its output on the new computer. A failure before joining can rerun the same command while it is unexpired.
- Used, but the device is missing or offline in `device.list`: the join was accepted and the device is still connecting.

## Troubleshooting a join

On the new computer, `zork mesh status` shows the join progress label, the attempt count and when the next retry happens, plus its direct and relay addresses. The join retries by itself; let it continue instead of running the command again. Most stuck joins are network problems on the new computer: no internet access, a proxy or firewall blocking outbound connections, or no reachable relay. Fix the cause on that computer, then check `mesh.invites` and `device.list` again. Diagnose one cause at a time and do not reset devices that are already healthy members.
