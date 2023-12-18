git push
if (-not $?)
{
    throw 'Native Failure'
}

# copy the tree to the WSL file system to improve compile times
wsl rsync --delete -av /mnt/c/Users/fenhl/git/github.com/midoshouse/midos.house/stage/ /home/fenhl/wslgit/github.com/midoshouse/midos.house/ --exclude target
if (-not $?)
{
    throw 'Native Failure'
}

wsl env -C /home/fenhl/wslgit/github.com/midoshouse/midos.house @args cargo build --release --target=x86_64-unknown-linux-musl
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

scp .\target\wsl\release\midos-house midos.house:bin/midos-house-next
if (-not $?)
{
    throw 'Native Failure'
}

ssh midos.house /opt/git/github.com/midoshouse/midos.house/main/assets/deploy-local.sh
if (-not $?)
{
    throw 'Native Failure'
}
