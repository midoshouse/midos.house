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

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO events (series, event, display_name, listed, short_name, team_config, discord_guild, discord_scheduling_channel, discord_race_room_channel, discord_race_results_channel, discord_organizer_channel) VALUES (''sco'', ''2026'', ''SlugCentral Open 2026'', TRUE, ''Slug Open'', ''slugopen'', 987565688820469781, 1487449014902194180, 1487449217344475346, 1487449279269179534, 1487449157655335102);"'
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

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO teams (id, series, event, name, racetime_slug) VALUES (-4960871259971959700, ''sco'', ''2026'', ''SlugCentral Open test team 1'', ''slugcentral-open-test-team-1'');"'
if (-not $?) {
    throw 'Native Failure'
}

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO team_members (team, member, status, role, hard_settings_ok, mq_ok) VALUES (-4960871259971959700, -3874943390487736167, ''created'', ''power'', TRUE, TRUE);"'
if (-not $?) {
    throw 'Native Failure'
}

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO team_members (team, member, status, role, hard_settings_ok, mq_ok) VALUES (-4960871259971959700, 137149793105548454, ''confirmed'', ''wisdom'', TRUE, FALSE);"'
if (-not $?) {
    throw 'Native Failure'
}

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO team_members (team, member, status, role, hard_settings_ok, mq_ok) VALUES (-4960871259971959700, 6193685995567200830, ''confirmed'', ''courage'', FALSE, FALSE);"'
if (-not $?) {
    throw 'Native Failure'
}

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO teams (id, series, event, name, racetime_slug) VALUES (-3742612991024895536, ''sco'', ''2026'', ''SlugCentral Open test team 2'', ''slugcentral-open-test-team-2'');"'
if (-not $?) {
    throw 'Native Failure'
}

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO team_members (team, member, status, role, hard_settings_ok, mq_ok) VALUES (-3742612991024895536, 8175787319745695527, ''created'', ''power'', TRUE, TRUE);"'
if (-not $?) {
    throw 'Native Failure'
}

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO team_members (team, member, status, role, hard_settings_ok, mq_ok) VALUES (-3742612991024895536, 2728112804012319386, ''confirmed'', ''wisdom'', TRUE, FALSE);"'
if (-not $?) {
    throw 'Native Failure'
}

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO team_members (team, member, status, role, hard_settings_ok, mq_ok) VALUES (-3742612991024895536, -2871260142009583217, ''confirmed'', ''courage'', FALSE, FALSE);"'
if (-not $?) {
    throw 'Native Failure'
}

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO discord_roles (guild, racetime_team, id) VALUES (987565688820469781, ''slugcentral-open-test-team-1'', 1487421659597111539);"'
if (-not $?) {
    throw 'Native Failure'
}

ssh midos.house sudo -u mido psql fados_house -c '"insert INTO discord_roles (guild, racetime_team, id) VALUES (987565688820469781, ''slugcentral-open-test-team-2'', 1487448439099756737);"'
if (-not $?) {
    throw 'Native Failure'
}
