#!/bin/sh
set -e
if command -v systemctl >/dev/null 2>&1; then
  systemctl stop pertisk-mgmt 2>/dev/null || true
  systemctl disable pertisk-mgmt 2>/dev/null || true
fi
