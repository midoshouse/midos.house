ssh midos.house env -C /opt/git/github.com/racetimeGG/racetime-app/main git pull
if (-not $?) {
    throw 'Native Failure'
}

ssh midos.house env -C /opt/git/github.com/racetimeGG/racetime-app/main docker-compose up --build -d
if (-not $?) {
    throw 'Native Failure'
}

ssh -t midos.house env -C /opt/git/github.com/racetimeGG/racetime-app/main docker-compose exec racetime.web python manage.py migrate
if (-not $?) {
    throw 'Native Failure'
}

ssh midos.house "sudo -u postgres psql -c 'DROP DATABASE fados_house;'"
if (-not $?) {
    throw 'Native Failure'
}

ssh midos.house "sudo -u postgres psql -c 'CREATE DATABASE fados_house;'"
if (-not $?) {
    throw 'Native Failure'
}

ssh midos.house "sh -c 'sudo -u postgres pg_dump -s midos_house | sudo -u postgres psql fados_house'"
if (-not $?) {
    throw 'Native Failure'
}

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO users (id, racetime_id, racetime_display_name, discord_id, discord_display_name, discord_username, display_source, is_archivist) VALUES (-3874943390487736167, ''5BRGVMd30E368Lzv'', ''Fenhl'', 86841168427495424, ''Fenhl'', ''fenhl'', ''racetime'', TRUE);"'
if (-not $?) {
    throw 'Native Failure'
}

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO api_keys (key, user_id, mw_admin) VALUES (''IAjThkPzhBOiwon7WLMvIavCaEhApHJI'', -3874943390487736167, TRUE);"'
if (-not $?) {
    throw 'Native Failure'
}

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO users (id, discord_id, discord_display_name, discord_username, display_source, is_archivist) VALUES (-683803002234927632, 187048694539878401, ''Xopar'', ''xopar'', ''discord'', FALSE);"'
if (-not $?) {
    throw 'Native Failure'
}

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO users (id, racetime_id, racetime_display_name, display_source, is_archivist) VALUES (1, ''K517W0dv3EL82Jm6'', ''Captain Falcon'', ''racetime'', FALSE);"'
if (-not $?) {
    throw 'Native Failure'
}

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO users (id, racetime_id, racetime_display_name, display_source, is_archivist) VALUES (2, ''5qZ10Aj1VdQgPbX3'', ''Diddy Kong'', ''racetime'', FALSE);"'
if (-not $?) {
    throw 'Native Failure'
}

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO users (id, racetime_id, racetime_display_name, display_source, is_archivist) VALUES (3, ''nKPa61EmYEyz0b7X'', ''Little Mac'', ''racetime'', FALSE);"'
if (-not $?) {
    throw 'Native Failure'
}

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO users (id, racetime_id, racetime_display_name, display_source, is_archivist) VALUES (4, ''XNprgAEB0j2yRPOw'', ''Charizard'', ''racetime'', FALSE);"'
if (-not $?) {
    throw 'Native Failure'
}

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO users (id, racetime_id, racetime_display_name, display_source, is_archivist) VALUES (5, ''mXBVxgoA2jz6MZO5'', ''Duck Hunt'', ''racetime'', FALSE);"'
if (-not $?) {
    throw 'Native Failure'
}

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO events (series, event, display_name, listed, short_name, team_config, discord_guild) VALUES (''sco'', ''2026'', ''SlugCentral Open 2026'', TRUE, ''Slug Open'', ''slugopen'', 987565688820469781);"'
if (-not $?) {
    throw 'Native Failure'
}

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO users (id, racetime_id, racetime_display_name, discord_id, discord_display_name, display_source, racetime_discriminator, discord_username) VALUES (6193685995567200830, ''z9VAe5EgYjRvB42w'', ''Kirby'', 289538227071877120, ''TeaGrenadier'', ''discord'', 3830, ''teagrenadier'');"'
if (-not $?) {
    throw 'Native Failure'
}

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO users (id, racetime_id, racetime_display_name, discord_id, discord_display_name, display_source, racetime_discriminator, discord_username) VALUES (137149793105548454, ''VXakOPonPdvDrB8n'', ''Ganondorf'', 212413638860996609, ''BearKofca'', ''discord'', 7849, ''bearkofca'');"'
if (-not $?) {
    throw 'Native Failure'
}

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO users (id, racetime_id, racetime_display_name, discord_id, discord_display_name, display_source, racetime_discriminator, discord_username) VALUES (8175787319745695527, ''Pl87GRox6Qd625zg'', ''Samus'', 691236965630083072, ''ksinjah'', ''discord'', 10, ''ksinjah'');"'
if (-not $?) {
    throw 'Native Failure'
}

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO users (id, racetime_id, racetime_display_name, discord_id, discord_display_name, display_source, racetime_discriminator, discord_username) VALUES (-2871260142009583217, ''gAzO07dQ5zdlG6Rv'', ''Yoshi'', 700863710981259265, ''melqwii'', ''discord'', 9537, ''melqwii'');"'
if (-not $?) {
    throw 'Native Failure'
}

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO users (id, racetime_id, racetime_display_name, discord_id, discord_display_name, display_source, racetime_discriminator, discord_username) VALUES (2728112804012319386, ''a9mKQGd8P4dAqJRg'', ''Shulk'', 228203986707021825, ''Tjongejonge'', ''discord'', 4199, ''tjongejonge'');"'
if (-not $?) {
    throw 'Native Failure'
}

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO organizers (series, event, organizer) VALUES (''sco'', ''2026'', -3874943390487736167);"'
if (-not $?) {
    throw 'Native Failure'
}
