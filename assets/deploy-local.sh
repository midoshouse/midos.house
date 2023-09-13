#!/bin/sh

if systemctl is-active midos-house; then
    sudo -u mido /usr/local/share/midos-house/bin/midos-house prepare-stop
fi

set -e

sudo systemctl stop midos-house
env -C /opt/git/github.com/midoshouse/midos.house/main git pull
sudo chown mido:www-data bin/midos-house-next
sudo chmod +x bin/midos-house-next
sudo mv bin/midos-house-next /usr/local/share/midos-house/bin/midos-house
sudo systemctl start midos-house
