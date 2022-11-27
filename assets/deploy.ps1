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

#TODO warn if there are any active races on racetime.gg/ootr that should be handled by Mido
#TODO stop racetime.gg room handler to delay handling new races until after the restart

ssh midos.house sudo systemctl stop midos-house
ThrowOnNativeFailure

ssh midos.house env -C /opt/git/github.com/midoshouse/midos.house/master git pull
ThrowOnNativeFailure

scp .\target\release\midos-house midos.house:bin/midos-house
ThrowOnNativeFailure

ssh midos.house sudo systemctl start midos-house
ThrowOnNativeFailure
