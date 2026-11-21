#!/usr/bin/env bash
# Install/refresh the DeepSeek Price Clock GNOME extension for the current user.
set -euo pipefail

UUID="deepseek-price-clock@local"
SRC="$(cd "$(dirname "$0")" && pwd)"
DEST="${HOME}/.local/share/gnome-shell/extensions/${UUID}"

rm -rf "${DEST}"
mkdir -p "${DEST}/schemas"
cp "${SRC}/extension.js" "${SRC}/price.js" "${SRC}/metadata.json" \
    "${SRC}/stylesheet.css" "${DEST}/"
cp "${SRC}/schemas/"*.xml "${DEST}/schemas/"
glib-compile-schemas "${DEST}/schemas"

gnome-extensions enable "${UUID}" || true

cat <<'EOF'

Installed. If this is the first install, log out and back in (Wayland cannot
reload the shell), then run:

  gnome-extensions enable deepseek-price-clock@local

Already installed before? Log out and back in to pick up the changes, or on
X11 press Alt+F2, type "r" and hit Enter.
EOF
