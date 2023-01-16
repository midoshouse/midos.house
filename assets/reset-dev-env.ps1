function ThrowOnNativeFailure {
    if (-not $?)
    {
        throw 'Native Failure'
    }
}

ssh midos.house env -C /opt/git/github.com/racetimeGG/racetime-app/master docker-compose up --build -d
ThrowOnNativeFailure

ssh midos.house env -C /opt/git/github.com/racetimeGG/racetime-app/master docker-compose exec racetime.web python manage.py migrate
ThrowOnNativeFailure

ssh midos.house sudo -u postgres psql -c 'DROP DATABASE fados_house; CREATE DATABASE fados_house;'
ThrowOnNativeFailure

ssh midos.house sh -c 'sudo -u postgres pg_dump -s midos_house | sudo -u postgres psql fados_house'
ThrowOnNativeFailure

ssh midos.house sudo -u mido psql fados_house -c 'INSERT INTO users (id, discord_id, discord_display_name, display_source, is_archivist) VALUES (-3874943390487736167, 86841168427495424, ''Fenhl'', ''discord'', TRUE);'
ThrowOnNativeFailure
