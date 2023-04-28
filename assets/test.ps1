function ThrowOnNativeFailure {
    if (-not $?)
    {
        throw 'Native Failure'
    }
}

# copy the tree to the WSL file system to improve compile times
wsl rsync --delete -av /mnt/c/Users/fenhl/git/github.com/midoshouse/midos.house/stage/ /home/fenhl/wslgit/github.com/midoshouse/midos.house/ --exclude target
ThrowOnNativeFailure

wsl env -C /home/fenhl/wslgit/github.com/midoshouse/midos.house cargo build --target=x86_64-unknown-linux-musl
ThrowOnNativeFailure

wsl cp /home/fenhl/wslgit/github.com/midoshouse/midos.house/target/x86_64-unknown-linux-musl/debug/midos-house /mnt/c/Users/fenhl/git/github.com/midoshouse/midos.house/stage/target/wsl/debug/midos-house
ThrowOnNativeFailure

ssh midos.house sudo -u mido killall -9 midos-house-dev

.\assets\reset-dev-env.ps1

scp .\target\wsl\debug\midos-house midos.house:bin/midos-house-dev
ThrowOnNativeFailure

ssh midos.house chmod +x bin/midos-house-dev
ThrowOnNativeFailure

ssh midos.house sudo -u mido env -C /opt/git/github.com/midoshouse/midos.house/master /home/fenhl/bin/midos-house-dev @args
ThrowOnNativeFailure
