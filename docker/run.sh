#!/usr/bin/env bash
# Run a command (default: docker/run-all.sh) in the `mvm-env` image with the
# repo mounted at /work. Proxy settings, when present, are passed through.
set -euo pipefail
cd "$(dirname "$0")/.."
args=(--rm -v "$PWD":/work -w /work)
for v in HTTPS_PROXY HTTP_PROXY NO_PROXY https_proxy http_proxy no_proxy; do
  [ -n "${!v:-}" ] && args+=(-e "$v=${!v}")
done
[ -n "${HTTPS_PROXY:-}${https_proxy:-}" ] && args+=(--network host)
[ -t 1 ] && args+=(-t)
docker run "${args[@]}" mvm-env "${@:-docker/run-all.sh}"
