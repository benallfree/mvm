#!/bin/sh
# Puts the Xtensa toolchain from espup on PATH, then runs the command.
set -e
. /opt/export-esp.sh
exec "$@"
