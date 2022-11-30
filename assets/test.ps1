function ThrowOnNativeFailure {
    if (-not $?)
    {
        throw 'Native Failure'
    }
}

wsl cargo build
ThrowOnNativeFailure

scp .\target\debug\midos-house midos.house:bin/midos-house-dev
ThrowOnNativeFailure

ssh midos.house chmod +x bin/midos-house-dev
ThrowOnNativeFailure

ssh midos.house sudo -u mido env -C /opt/git/github.com/midoshouse/midos.house/master /home/fenhl/bin/midos-house-dev @args
ThrowOnNativeFailure
