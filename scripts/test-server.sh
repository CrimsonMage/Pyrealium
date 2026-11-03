#!/bin/sh
# Start a local Pyrealium for client testing: writes a settings file into a state directory and runs
# `pyrealium run` on it. Clients' test tooling calls this script instead of writing the server's
# settings themselves, so the settings format stays in this repository.
#
#   scripts/test-server.sh [--account NAME] [--training-kit B] [--test-map-travel B]
#                          [--lifestones B] [--portals B] [--initialize-ui B] [--auth-data X]
#
# PYREALIUM_BIN    the server executable; default target/release/pyrealium when built, else
#             `pyrealium` on PATH
# AC2_DAT_DIR      the 7230 DATs; default ../dataset-7230
# PYREALIUM_STATE  the settings file, database, run directory and logs; default
#             ~/.local/state/pyrealium-test. Each state directory has its own database file
#             (pyrealium.db); `pyrealium db drop --config "$PYREALIUM_STATE/pyrealium.toml"` removes it
# PYREALIUM_BIND   game address; default 127.0.0.1:9000
# PYREALIUM_ACCOUNTS_BIND  with ticket logins (--auth-data), the account service's address;
#             default 127.0.0.1:19400
# PYREALIUM_EXTRA_WORLD  a file of placements (world table rows, v4 columns) added to the world
#             (`[world] content_overlay`)
# PYREALIUM_TRAINING_WORLD  1 keeps the training kit's authored placements (`[world] training_world`);
#             default off
# PYREALIUM_SHELTER_QUEST  0 leaves out Rescue the Shelter Hold's placements (`[world] shelter_quest`)
# PYREALIUM_GUN_BIND  the GUN service address; default off
# PYREALIUM_DEV_TRADESKILLS  the tradeskills development profile; default on (0 turns it off)
#
# Options: --account (the developer account, default local), --training-kit (default true),
# --test-map-travel (default true), --lifestones, --portals, --initialize-ui (default true), and
# --auth-data X (any X) for ticket logins from the account service instead of developer logins.
# Later options win.
#
# The server prints `Listening on` when it takes connections. Stop it with
# `pyrealium stop --config "$PYREALIUM_STATE/pyrealium.toml"` (or touch data/stop.request there).
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
if [ -n "${PYREALIUM_BIN:-}" ]; then bin=$PYREALIUM_BIN
elif [ -x "$root/target/release/pyrealium" ]; then bin=$root/target/release/pyrealium
else bin=pyrealium; fi
command -v "$bin" > /dev/null 2>&1 || {
  echo "no Pyrealium server executable '$bin': build it (cargo build --release) or set PYREALIUM_BIN" >&2; exit 1; }
state=${PYREALIUM_STATE:-${XDG_STATE_HOME:-$HOME/.local/state}/pyrealium-test}
bind=${PYREALIUM_BIND:-127.0.0.1:9000}
dats=${AC2_DAT_DIR:-$root/../dataset-7230}
[ -f "$dats/portal.dat" ] || { echo "no portal.dat in $dats: set AC2_DAT_DIR" >&2; exit 1; }
mkdir -p "$state/data"
state=$(cd "$state" && pwd)
dats=$(cd "$dats" && pwd)

account=local training_kit=true map_travel=true lifestones=true portals=true ui=true auth=developer
while [ $# -gt 0 ]; do
  [ $# -ge 2 ] || { echo "missing value for $1" >&2; exit 1; }
  case $1 in
    --account) account=$2 ;;
    --training-kit) training_kit=$2 ;;
    --test-map-travel) map_travel=$2 ;;
    --lifestones) lifestones=$2 ;;
    --portals) portals=$2 ;;
    --initialize-ui) ui=$2 ;;
    --auth-data) auth=tickets ;;
    *) echo "unknown option $1" >&2; exit 1 ;;
  esac
  shift 2
done

overlay=
if [ -n "${PYREALIUM_EXTRA_WORLD:-}" ]; then
  overlay=$state/extra-world.tsv
  { echo pyrealium-world-v4; grep -v '^pyrealium-world-v' "$PYREALIUM_EXTRA_WORLD"; } > "$overlay"
fi
toml() { printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'; }
training_world=false; [ "${PYREALIUM_TRAINING_WORLD:-0}" = 1 ] && training_world=true
shelter_quest=true; [ "${PYREALIUM_SHELTER_QUEST:-1}" = 0 ] && shelter_quest=false
{
  echo '[server]'
  echo "bind = \"$bind\""
  echo "auth = \"$auth\""
  echo "account = \"$(toml "$account")\""
  echo "game_host = \"${bind%:*}\""
  echo "dat_dir = \"$(toml "$dats")\""
  echo "run_dir = \"$(toml "$state/data")\""
  echo '[database]'
  echo "path = \"$(toml "$state/pyrealium.db")\""
  echo '[accounts]'
  echo "enabled = $( [ "$auth" = tickets ] && echo true || echo false )"
  echo "web_bind = \"${PYREALIUM_ACCOUNTS_BIND:-127.0.0.1:19400}\""
  echo '[world]'
  echo "training_world = $training_world"
  echo "shelter_quest = $shelter_quest"
  [ -n "$overlay" ] && echo "content_overlay = \"$(toml "$overlay")\""
  echo "initialize_ui = $ui"
  echo "training_kit = $training_kit"
  echo "lifestones = $lifestones"
  echo "portals = $portals"
  echo "test_map_travel = $map_travel"
  if [ -n "${PYREALIUM_GUN_BIND:-}" ] && [ "$PYREALIUM_GUN_BIND" != off ]; then
    echo '[gun]'
    echo "bind = \"$PYREALIUM_GUN_BIND\""
  fi
} > "$state/pyrealium.toml"

# The server reads its tradeskills development profile from the variable's presence.
if [ "${PYREALIUM_DEV_TRADESKILLS:-1}" = 0 ]; then unset PYREALIUM_DEV_TRADESKILLS; else export PYREALIUM_DEV_TRADESKILLS=${PYREALIUM_DEV_TRADESKILLS:-1}; fi
# A stop request left after a server exited would stop this one on its first pass.
rm -f "$state/data/stop.request"
exec "$bin" run --config "$state/pyrealium.toml"
