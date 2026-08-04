#!/bin/sh
set -e
getent group pertisk-mgmt >/dev/null 2>&1 || groupadd -r pertisk-mgmt
getent passwd pertisk-mgmt >/dev/null 2>&1 || \
  useradd -r -g pertisk-mgmt -d /var/lib/pertisk-mgmt -s /sbin/nologin \
    -c "Pertisk management" pertisk-mgmt
