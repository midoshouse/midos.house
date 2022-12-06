function ThrowOnNativeFailure {
    if (-not $?)
    {
        throw 'Native Failure'
    }
}

# copy the tree to the WSL file system to improve compile times
wsl rsync -av /mnt/c/Users/fenhl/git/github.com/midoshouse/midos.house/stage/ /home/fenhl/wslgit/github.com/midoshouse/midos.house/ --exclude target
ThrowOnNativeFailure

wsl env -C /home/fenhl/wslgit/github.com/midoshouse/midos.house cargo build
ThrowOnNativeFailure

wsl cp /home/fenhl/wslgit/github.com/midoshouse/midos.house/target/debug/midos-house /mnt/c/Users/fenhl/git/github.com/midoshouse/midos.house/stage/target/wsl/debug/midos-house
ThrowOnNativeFailure

scp .\target\wsl\debug\midos-house midos.house:bin/midos-house-dev
ThrowOnNativeFailure

ssh midos.house chmod +x bin/midos-house-dev
ThrowOnNativeFailure

ssh midos.house sudo -u mido env -C /opt/git/github.com/midoshouse/midos.house/master /home/fenhl/bin/midos-house-dev @args
ThrowOnNativeFailure
