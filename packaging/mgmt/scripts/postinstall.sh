#!/bin/sh
set -e
install -d -o pertisk-mgmt -g pertisk-mgmt -m 0750 /var/lib/pertisk-mgmt
install -d -o pertisk-mgmt -g pertisk-mgmt -m 0750 /var/lib/pertisk-mgmt/images
chown -R pertisk-mgmt:pertisk-mgmt /var/lib/pertisk-mgmt 2>/dev/null || true
chown root:pertisk-mgmt /etc/pertisk-mgmt/pertisk-mgmt.env 2>/dev/null || true
chmod 0640 /etc/pertisk-mgmt/pertisk-mgmt.env 2>/dev/null || true
if command -v systemctl >/dev/null 2>&1; then
  systemctl daemon-reload || true
  echo "Enable/start: systemctl enable --now pertisk-mgmt"
  echo "Upload cloud qcow2 in the UI (Images) or copy to /var/lib/pertisk-mgmt/images/ before Create Cluster."
fi
# Prism pulls qcow2/netcfg from this host on :18765 (pertisk-mgmt is not root).
if command -v firewall-cmd >/dev/null 2>&1 && firewall-cmd --state >/dev/null 2>&1; then
  firewall-cmd --quiet --permanent --add-port=18765/tcp || true
  firewall-cmd --quiet --reload || true
fi
