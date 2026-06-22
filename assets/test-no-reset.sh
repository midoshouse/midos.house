#!/bin/sh

set -e

ssh midos.house sudo -u mido env -C /opt/git/github.com/midoshouse/midos.house/build-dev git pull
ssh midos.house env -C /opt/git/github.com/midoshouse/midos.house/build-dev cargo build --jobs=-1 --release --features=dev

set +e

ssh midos.house sudo -u mido killall -9 midos-house-dev

set -e

ssh midos.house sudo -u mido cp /opt/git/github.com/midoshouse/midos.house/build-dev/target/release/midos-house /usr/local/share/midos-house/bin/midos-house-dev
ssh midos.house sudo -u mido env -C /opt/git/github.com/midoshouse/midos.house/build-dev /usr/local/share/midos-house/bin/midos-house-dev "$@"
