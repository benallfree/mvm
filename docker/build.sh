#!/usr/bin/env bash
# Build the `mvm-env` image.
#
# Behind a TLS-intercepting proxy, set MVM_EXTRA_CA to the proxy's CA bundle
# (in a Claude Code cloud session: /root/.ccr/ca-bundle.crt). When a proxy is
# configured in the environment, the build uses host networking and gets the
# proxy settings as build args. With neither, it is a plain `docker build`.
set -euo pipefail
cd "$(dirname "$0")/.."
cleanup() { rm -f docker/extra-ca/extra.crt; }
trap cleanup EXIT
[ -n "${MVM_EXTRA_CA:-}" ] && cp "$MVM_EXTRA_CA" docker/extra-ca/extra.crt
args=()
for v in HTTPS_PROXY HTTP_PROXY NO_PROXY https_proxy http_proxy no_proxy; do
  [ -n "${!v:-}" ] && args+=(--build-arg "$v=${!v}")
done
[ ${#args[@]} -gt 0 ] && args+=(--network host)
docker build "${args[@]}" -f docker/Dockerfile -t mvm-env "$@" .
