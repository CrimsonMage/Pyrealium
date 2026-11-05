#!/bin/sh
# Run a content generator (tools/author_*.py, tools/import_*.py, ...) on the content files it was
# written for. The content lives in content/content.sql as database rows; this writes its files
# into content/ (`pyrealium-content checkout`), runs the command, imports what the command left
# there back into content.sql (`pyrealium-content checkin`), and removes the written files again.
#
#   scripts/with-content-files.sh python3 tools/author_professions.py ...
#
# PYREALIUM_CONTENT_BIN  the pyrealium-content executable; default: built and run with cargo.
# Files the command adds that the database does not know (a new table file) stay in content/ and
# are listed: give them a table (crates/pyrealium/src/content_db/files.rs) first.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
[ "$#" -gt 0 ] || { echo "usage: scripts/with-content-files.sh COMMAND [ARGS...]" >&2; exit 2; }
if [ -n "${PYREALIUM_CONTENT_BIN:-}" ]; then tool() { "$PYREALIUM_CONTENT_BIN" "$@"; }
else tool() { (cd "$root" && cargo run -q -p pyrealium --bin pyrealium-content -- "$@"); }; fi

before=$(mktemp); written=$(mktemp); after=$(mktemp)
trap 'rm -f "$before" "$written" "$after"' EXIT
files() { ( cd "$root/content" && find . -type f | sort ); }
files > "$before"
tool checkout "$root/content" "$root/content"
files | comm -13 "$before" - > "$written"
status=0
( cd "$root" && "$@" ) || status=$?
if [ "$status" -eq 0 ]; then
  tool checkin "$root/content" "$root/content"
else
  echo "the command failed ($status); content.sql is unchanged" >&2
fi
# Remove what checkout wrote, whether or not it was checked in; list what the command added.
files > "$after"
while read -r file; do rm -f "$root/content/$file"; done < "$written"
added=$(sort -m "$before" "$written" | comm -13 - "$after" | grep -v -e '^./content.sql$' -e '^./content.sqlite$' || true)
[ -z "$added" ] || printf 'files the command added, not in the database (left in content/):\n%s\n' "$added" >&2
find "$root/content" -mindepth 1 -type d -empty -delete
exit "$status"
