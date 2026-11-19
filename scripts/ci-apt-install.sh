#!/usr/bin/env bash
# Install Ubuntu packages on CI runners: scripts/ci-apt-install.sh PACKAGE...
#
# The runner's apt mirror sometimes trickles (seconds per package), which kept
# plain `apt-get install` running past job timeouts. Downloads run separately
# so each retry resumes partial files, later attempts switch to
# archive.ubuntu.com, and .debs are kept in ~/apt-archives for an
# actions/cache step to restore on the next run. Same approach as ci.yml.
set -euo pipefail

archives="$HOME/apt-archives"
if compgen -G "$archives/*.deb" > /dev/null; then
  sudo cp "$archives"/*.deb /var/cache/apt/archives/
fi
apt_opts=(-o Acquire::Retries=3 -o Acquire::http::Timeout=30)
downloaded=
for attempt in 1 2 3 4; do
  if [ "$attempt" -ge 3 ]; then
    echo "http://archive.ubuntu.com/ubuntu/" | sudo tee /etc/apt/apt-mirrors.txt
  fi
  if timeout 180 sudo apt-get "${apt_opts[@]}" update &&
     timeout 240 sudo apt-get "${apt_opts[@]}" install -y --download-only "$@"; then
    downloaded=1
    break
  fi
  echo "apt download attempt $attempt failed; retrying"
  sleep 10
done
test -n "$downloaded"
sudo apt-get install -y --no-download "$@"
mkdir -p "$archives"
if compgen -G "/var/cache/apt/archives/*.deb" > /dev/null; then
  cp /var/cache/apt/archives/*.deb "$archives/"
fi
