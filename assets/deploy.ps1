function ThrowOnNativeFailure {
    if (-not $?)
    {
        throw 'Native Failure'
    }
}

git push
ThrowOnNativeFailure

# copy the tree to the WSL file system to improve compile times
wsl rsync --delete -av /mnt/c/Users/fenhl/git/github.com/midoshouse/midos.house/stage/ /home/fenhl/wslgit/github.com/midoshouse/midos.house/ --exclude target
ThrowOnNativeFailure

wsl env -C /home/fenhl/wslgit/github.com/midoshouse/midos.house cargo build --release --target=x86_64-unknown-linux-musl
ThrowOnNativeFailure

wsl cp /home/fenhl/wslgit/github.com/midoshouse/midos.house/target/x86_64-unknown-linux-musl/release/midos-house /mnt/c/Users/fenhl/git/github.com/midoshouse/midos.house/stage/target/wsl/release/midos-house
ThrowOnNativeFailure

scp .\target\wsl\release\midos-house midos.house:bin/midos-house-next
ThrowOnNativeFailure

ssh midos.house 'if systemctl is-active midos-house; then sudo -u mido bin/midos-house prepare-stop; fi'
ThrowOnNativeFailure

ssh midos.house sudo systemctl stop midos-house
ThrowOnNativeFailure

ssh midos.house env -C /opt/git/github.com/midoshouse/midos.house/master git pull
ThrowOnNativeFailure

ssh midos.house mv bin/midos-house-next bin/midos-house
ThrowOnNativeFailure

ssh midos.house sudo systemctl start midos-house
ThrowOnNativeFailure
