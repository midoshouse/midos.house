function ThrowOnNativeFailure {
    if (-not $?)
    {
        throw 'Native Failure'
    }
}

git push
ThrowOnNativeFailure

wsl cargo build --release
ThrowOnNativeFailure

ssh midos.house sudo -u mido bin/midos-house prepare-stop
ThrowOnNativeFailure

ssh midos.house sudo systemctl stop midos-house
ThrowOnNativeFailure

ssh midos.house env -C /opt/git/github.com/midoshouse/midos.house/master git pull
ThrowOnNativeFailure

scp .\target\release\midos-house midos.house:bin/midos-house
ThrowOnNativeFailure

ssh midos.house sudo systemctl start midos-house
ThrowOnNativeFailure
