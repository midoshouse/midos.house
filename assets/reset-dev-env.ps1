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

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO events (series, event, display_name, start, listed, short_name, show_qualifier_times, default_game_count, min_schedule_notice, discord_guild, discord_race_room_channel, discord_race_results_channel, discord_organizer_channel, discord_scheduling_channel, team_config) VALUES (''mw'', ''3'', ''3rd Multiworld Tournament'', TIMESTAMPTZ ''2022-09-13 16:00:00Z'', TRUE, ''MW S3'', TRUE, 1, INTERVAL ''00:00:00'', 987565688820469781, 1064510809797038171, 1064510848833441833, 1064510908736491562, 1026055492788834324, ''multiworld'');"'
if (-not $?) {
    throw 'Native Failure'
}

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO organizers (series, event, organizer) VALUES (''mw'', ''3'', -3874943390487736167);"'
if (-not $?) {
    throw 'Native Failure'
}

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO teams (id, series, event, name, racetime_slug, restream_consent, plural_name) VALUES (1, ''mw'', ''3'', ''test team'', ''test-team'', TRUE, FALSE);"'
if (-not $?) {
    throw 'Native Failure'
}

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO team_members (team, member, status, role) VALUES (1, -3874943390487736167, ''created'', ''power'');"'
if (-not $?) {
    throw 'Native Failure'
}

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO team_members (team, member, status, role) VALUES (1, 1, ''confirmed'', ''wisdom'');"'
if (-not $?) {
    throw 'Native Failure'
}

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO team_members (team, member, status, role) VALUES (1, 2, ''confirmed'', ''courage'');"'
if (-not $?) {
    throw 'Native Failure'
}

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO teams (id, series, event, name, racetime_slug, restream_consent, plural_name) VALUES (2, ''mw'', ''3'', ''test team 2'', ''test-team-2'', TRUE, FALSE);"'
if (-not $?) {
    throw 'Native Failure'
}

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO team_members (team, member, status, role) VALUES (2, 3, ''created'', ''power'');"'
if (-not $?) {
    throw 'Native Failure'
}

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO team_members (team, member, status, role) VALUES (2, 4, ''confirmed'', ''wisdom'');"'
if (-not $?) {
    throw 'Native Failure'
}

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO team_members (team, member, status, role) VALUES (2, 5, ''confirmed'', ''courage'');"'
if (-not $?) {
    throw 'Native Failure'
}

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO events (series, event, display_name, start, listed, short_name, show_qualifier_times, default_game_count, min_schedule_notice, discord_guild, discord_race_room_channel, discord_race_results_channel, discord_organizer_channel, discord_scheduling_channel, team_config) VALUES (''tfb'', ''2'', ''Triforce Blitz Season 2 Tournament'', TIMESTAMPTZ ''2023-04-08 19:00:00Z'', TRUE, ''TFB S2'', TRUE, 3, INTERVAL ''00:00:00'', 987565688820469781, 1064510809797038171, 1064510848833441833, 1064510908736491562, 1026055492788834324, ''solo'');"'
if (-not $?) {
    throw 'Native Failure'
}

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO organizers (series, event, organizer) VALUES (''tfb'', ''2'', -3874943390487736167);"'
if (-not $?) {
    throw 'Native Failure'
}

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO teams (id, series, event, restream_consent, plural_name) VALUES (3, ''tfb'', ''2'', TRUE, FALSE);"'
if (-not $?) {
    throw 'Native Failure'
}

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO team_members (team, member, status, role) VALUES (3, -3874943390487736167, ''created'', ''none'');"'
if (-not $?) {
    throw 'Native Failure'
}

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO teams (id, series, event, restream_consent, plural_name) VALUES (4, ''tfb'', ''2'', TRUE, FALSE);"'
if (-not $?) {
    throw 'Native Failure'
}

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO team_members (team, member, status, role) VALUES (4, 1, ''created'', ''none'');"'
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
