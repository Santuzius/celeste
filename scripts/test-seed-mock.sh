#!/usr/bin/env bash
# Seed the isolated test instance (scripts/test-instance.sh) with offline mock remotes:
#   LocalMock   rclone `alias` onto a local dir → real, offline two-way sync between two dirs under $T/mock
#   OfflineMock rclone `webdav` at an unreachable port → listing fails (Error state)
#   ProtonMock  native Proton remote without a session → "Not signed in" state
# Run after the test instance has started once (so the DB exists) and has been stopped again.
set -euo pipefail
T="${CELESTE_TEST_DIR:-/tmp/celeste-test}"
DB="$T/home/.local/share/celeste/data.sqlite"
[ -f "$DB" ] || { echo "no $DB — start scripts/test-instance.sh once first"; exit 1; }
SQLITE="$(nix-build '<nixpkgs>' -A sqlite.bin --no-out-link 2>/dev/null)/bin/sqlite3"

mkdir -p "$T/run/celeste" "$T/mock/local/Documents/Notes" "$T/mock/local/Photos" "$T/mock/remote/Documents" "$T/mock/local/Offline"
cat > "$T/run/celeste/rclone.conf" <<EOF
[LocalMock]
type = alias
remote = $T/mock/remote

[OfflineMock]
type = webdav
url = http://127.0.0.1:9/
vendor = other
EOF

for i in $(seq 1 24); do echo "note $i" > "$T/mock/local/Documents/Notes/note-$i.md"; done
echo "draft" > "$T/mock/local/Documents/report.odt"
for i in $(seq 1 6); do head -c 20000 /dev/urandom > "$T/mock/local/Photos/IMG_$i.jpg"; done
echo "from the cloud" > "$T/mock/remote/Documents/shared.txt"

"$SQLITE" "$DB" <<EOF
DELETE FROM sync_dirs WHERE remote_id IN (SELECT id FROM remotes WHERE name IN ('LocalMock', 'OfflineMock', 'ProtonMock'));
DELETE FROM remotes WHERE name IN ('LocalMock', 'OfflineMock', 'ProtonMock');
INSERT INTO remotes (name, sync_interval_seconds, enabled, backend, session_path) VALUES
  ('LocalMock', 30, 1, 'rclone', NULL),
  ('OfflineMock', 60, 1, 'rclone', NULL),
  ('ProtonMock', 15, 1, 'native-proton', NULL);
INSERT INTO sync_dirs (remote_id, local_path, remote_path) VALUES
  ((SELECT id FROM remotes WHERE name='LocalMock'), '$T/mock/local/Documents', 'Documents'),
  ((SELECT id FROM remotes WHERE name='LocalMock'), '$T/mock/local/Photos', 'Photos'),
  ((SELECT id FROM remotes WHERE name='OfflineMock'), '$T/mock/local/Offline', 'Backup'),
  ((SELECT id FROM remotes WHERE name='ProtonMock'), '$T/mock/local/Proton', 'Documents');
EOF
echo "seeded $DB"
