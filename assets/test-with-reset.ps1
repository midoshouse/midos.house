ssh midos.house sudo -u mido env -C /opt/git/github.com/midoshouse/midos.house/build-dev git pull
if (-not $?)
{
    throw 'Native Failure'
}

ssh midos.house env -C /opt/git/github.com/midoshouse/midos.house/build-dev cargo build --release --features=dev
if (-not $?)
{
    throw 'Native Failure'
}

ssh midos.house sudo -u mido killall -9 midos-house-dev

.\assets\reset-dev-env.ps1

ssh midos.house sudo -u mido cp /opt/git/github.com/midoshouse/midos.house/build-dev/target/release/midos-house /usr/local/share/midos-house/bin/midos-house-dev
if (-not $?)
{
    throw 'Native Failure'
}

ssh midos.house sudo -u mido env -C /opt/git/github.com/midoshouse/midos.house/build-dev /usr/local/share/midos-house/bin/midos-house-dev @args
if (-not $?)
{
    throw 'Native Failure'
}
