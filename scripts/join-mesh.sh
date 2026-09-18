#!/bin/sh
# Run on the device being added. The native command discovers existing Stations
# and asks the selected Station to redeem the invitation.
set -eu
if [ "$#" -lt 1 ]; then
  echo 'Usage: join-mesh.sh INVITATION [--data DIRECTORY] [--name DEVICE_NAME]' >&2
  exit 2
fi
if command -v zork >/dev/null 2>&1 && zork capabilities 2>/dev/null | grep -q '"mesh_join":1'; then
  exec zork mesh join "$@"
fi
base=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
exec sh "$base/install.sh" -- mesh join "$@"
