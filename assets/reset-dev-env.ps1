function ThrowOnNativeFailure {
    if (-not $?) {
        throw 'Native Failure'
    }
}

ssh midos.house env -C /opt/git/github.com/racetimeGG/racetime-app/main git pull
ThrowOnNativeFailure

ssh midos.house env -C /opt/git/github.com/racetimeGG/racetime-app/main docker compose up --build -d
ThrowOnNativeFailure

ssh -t midos.house env -C /opt/git/github.com/racetimeGG/racetime-app/main docker compose exec racetime.web python manage.py migrate
ThrowOnNativeFailure

ssh midos.house "sudo -u postgres psql -c 'DROP DATABASE fados_house;'"
ThrowOnNativeFailure

ssh midos.house "sudo -u postgres psql -c 'CREATE DATABASE fados_house;'"
ThrowOnNativeFailure

ssh midos.house "sh -c 'sudo -u postgres pg_dump -s midos_house | sudo -u postgres psql fados_house'"
ThrowOnNativeFailure

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO users (id, racetime_id, racetime_display_name, discord_id, discord_display_name, discord_username, display_source, is_archivist) VALUES (-3874943390487736167, ''5BRGVMd30E368Lzv'', ''Fenhl'', 86841168427495424, ''Fenhl'', ''fenhl'', ''racetime'', TRUE);"'
ThrowOnNativeFailure

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO api_keys (key, user_id, mw_admin) VALUES (''IAjThkPzhBOiwon7WLMvIavCaEhApHJI'', -3874943390487736167, TRUE);"'
ThrowOnNativeFailure

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO users (id, discord_id, discord_display_name, discord_username, display_source, is_archivist) VALUES (-683803002234927632, 187048694539878401, ''Xopar'', ''xopar'', ''discord'', FALSE);"'
ThrowOnNativeFailure

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO users (id, racetime_id, racetime_display_name, display_source, is_archivist) VALUES (1, ''K517W0dv3EL82Jm6'', ''Captain Falcon'', ''racetime'', FALSE);"'
ThrowOnNativeFailure

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO users (id, racetime_id, racetime_display_name, display_source, is_archivist) VALUES (2, ''5qZ10Aj1VdQgPbX3'', ''Diddy Kong'', ''racetime'', FALSE);"'
ThrowOnNativeFailure

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO users (id, racetime_id, racetime_display_name, display_source, is_archivist) VALUES (3, ''nKPa61EmYEyz0b7X'', ''Little Mac'', ''racetime'', FALSE);"'
ThrowOnNativeFailure

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO users (id, racetime_id, racetime_display_name, display_source, is_archivist) VALUES (4, ''XNprgAEB0j2yRPOw'', ''Charizard'', ''racetime'', FALSE);"'
ThrowOnNativeFailure

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO users (id, racetime_id, racetime_display_name, display_source, is_archivist) VALUES (5, ''mXBVxgoA2jz6MZO5'', ''Duck Hunt'', ''racetime'', FALSE);"'
ThrowOnNativeFailure

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO events (series, event, display_name, start, listed, short_name, show_qualifier_times, default_game_count, min_schedule_notice, discord_guild, discord_race_room_channel, discord_race_results_channel, discord_organizer_channel, discord_scheduling_channel, team_config) VALUES (''mw'', ''3'', ''3rd Multiworld Tournament'', TIMESTAMPTZ ''2022-09-13 16:00:00Z'', TRUE, ''MW S3'', TRUE, 1, INTERVAL ''00:00:00'', 987565688820469781, 1064510809797038171, 1064510848833441833, 1064510908736491562, 1026055492788834324, ''multiworld'');"'
ThrowOnNativeFailure

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO organizers (series, event, organizer) VALUES (''mw'', ''3'', -3874943390487736167);"'
ThrowOnNativeFailure

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO teams (id, series, event, name, racetime_slug, restream_consent, plural_name) VALUES (1, ''mw'', ''3'', ''test team'', ''test-team'', TRUE, FALSE);"'
ThrowOnNativeFailure

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO team_members (team, member, status, role) VALUES (1, -3874943390487736167, ''created'', ''power'');"'
ThrowOnNativeFailure

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO team_members (team, member, status, role) VALUES (1, 1, ''confirmed'', ''wisdom'');"'
ThrowOnNativeFailure

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO team_members (team, member, status, role) VALUES (1, 2, ''confirmed'', ''courage'');"'
ThrowOnNativeFailure

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO teams (id, series, event, name, racetime_slug, restream_consent, plural_name) VALUES (2, ''mw'', ''3'', ''test team 2'', ''test-team-2'', TRUE, FALSE);"'
ThrowOnNativeFailure

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO team_members (team, member, status, role) VALUES (2, 3, ''created'', ''power'');"'
ThrowOnNativeFailure

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO team_members (team, member, status, role) VALUES (2, 4, ''confirmed'', ''wisdom'');"'
ThrowOnNativeFailure

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO team_members (team, member, status, role) VALUES (2, 5, ''confirmed'', ''courage'');"'
ThrowOnNativeFailure

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO events (series, event, display_name, start, listed, short_name, show_qualifier_times, default_game_count, min_schedule_notice, discord_guild, discord_race_room_channel, discord_race_results_channel, discord_organizer_channel, discord_scheduling_channel, team_config) VALUES (''tfb'', ''2'', ''Triforce Blitz Season 2 Tournament'', TIMESTAMPTZ ''2023-04-08 19:00:00Z'', TRUE, ''TFB S2'', TRUE, 3, INTERVAL ''00:00:00'', 987565688820469781, 1064510809797038171, 1064510848833441833, 1064510908736491562, 1026055492788834324, ''solo'');"'
ThrowOnNativeFailure

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO organizers (series, event, organizer) VALUES (''tfb'', ''2'', -3874943390487736167);"'
ThrowOnNativeFailure

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO teams (id, series, event, restream_consent, plural_name) VALUES (3, ''tfb'', ''2'', TRUE, FALSE);"'
ThrowOnNativeFailure

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO team_members (team, member, status, role) VALUES (3, -3874943390487736167, ''created'', ''none'');"'
ThrowOnNativeFailure

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO teams (id, series, event, restream_consent, plural_name) VALUES (4, ''tfb'', ''2'', TRUE, FALSE);"'
ThrowOnNativeFailure

ssh midos.house sudo -u mido psql fados_house -c '"INSERT INTO team_members (team, member, status, role) VALUES (4, 1, ''created'', ''none'');"'
ThrowOnNativeFailure
