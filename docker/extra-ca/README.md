Extra CA certificates (`*.crt`) to trust inside the image. Empty normally.
`docker/build.sh` copies `$MVM_EXTRA_CA` here for the build (needed behind a
TLS-intercepting proxy, e.g. a Claude Code cloud session) and removes it after.
