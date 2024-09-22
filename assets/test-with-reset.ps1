# copy the tree to the WSL file system to improve compile times
wsl rsync --delete -av /mnt/c/Users/fenhl/git/github.com/midoshouse/midos.house/stage/ /home/fenhl/wslgit/github.com/midoshouse/midos.house/ --exclude target
if (-not $?)
{
    throw 'Native Failure'
}

wsl env -C /home/fenhl/wslgit/github.com/midoshouse/midos.house cargo build --target=x86_64-unknown-linux-musl --release --features=dev
if (-not $?)
{
    throw 'Native Failure'
}

wsl mkdir -p /mnt/c/Users/fenhl/git/github.com/midoshouse/midos.house/stage/target/wsl/release
if (-not $?)
{
    throw 'Native Failure'
}

wsl cp /home/fenhl/wslgit/github.com/midoshouse/midos.house/target/x86_64-unknown-linux-musl/release/midos-house /mnt/c/Users/fenhl/git/github.com/midoshouse/midos.house/stage/target/wsl/release/midos-house
if (-not $?)
{
    throw 'Native Failure'
}

ssh midos.house sudo -u mido killall -9 midos-house-dev

.\assets\reset-dev-env.ps1

scp .\target\wsl\release\midos-house midos.house:bin/midos-house-dev
if (-not $?)
{
    throw 'Native Failure'
}

ssh midos.house sudo chown mido:www-data bin/midos-house-dev
if (-not $?)
{
    throw 'Native Failure'
}

ssh midos.house sudo chmod +x bin/midos-house-dev
if (-not $?)
{
    throw 'Native Failure'
}

ssh midos.house sudo mv bin/midos-house-dev /usr/local/share/midos-house/bin/midos-house-dev
if (-not $?)
{
    throw 'Native Failure'
}

ssh midos.house sudo -u mido env -C /opt/git/github.com/midoshouse/midos.house/main /usr/local/share/midos-house/bin/midos-house-dev --env=dev @args
if (-not $?)
{
    throw 'Native Failure'
}
