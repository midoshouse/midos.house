#!/bin/sh

if systemctl is-active midos-house; then
    sudo -u mido bin/midos-house prepare-stop
fi

set -e

sudo systemctl stop midos-house
env -C /opt/git/github.com/midoshouse/midos.house/master git pull
mv bin/midos-house-next bin/midos-house
chmod +x bin/midos-house
sudo systemctl start midos-house
