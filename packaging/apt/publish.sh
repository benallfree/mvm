#!/usr/bin/env bash
# Adds an mvm release's .deb files to the apt repository checkout and
# re-signs its index (implementation plan 0.3b).
# Usage: publish.sh <dist dir holding the .deb files> <apt repo checkout>
# Needs apt-ftparchive (apt-utils) and the signing key in gpg's keyring.
set -euo pipefail
dist="$1"; repo="$2"
cd "$repo"
mkdir -p pool/main/m/mvm
cp "$dist"/mvm_*.deb pool/main/m/mvm/
for arch in amd64 arm64; do
  mkdir -p "dists/stable/main/binary-$arch"
  apt-ftparchive --arch "$arch" packages pool > "dists/stable/main/binary-$arch/Packages"
  gzip -kf "dists/stable/main/binary-$arch/Packages"
done
apt-ftparchive \
  -o APT::FTPArchive::Release::Origin=mvm \
  -o APT::FTPArchive::Release::Label=mvm \
  -o APT::FTPArchive::Release::Suite=stable \
  -o APT::FTPArchive::Release::Codename=stable \
  -o "APT::FTPArchive::Release::Architectures=amd64 arm64" \
  -o APT::FTPArchive::Release::Components=main \
  release dists/stable > dists/stable/Release
gpg --batch --yes --armor --detach-sign -o dists/stable/Release.gpg dists/stable/Release
gpg --batch --yes --clearsign -o dists/stable/InRelease dists/stable/Release
gpg --batch --yes --export -o mvm.gpg
touch .nojekyll
