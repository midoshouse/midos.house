#!/usr/bin/python3 -i

import datetime
import json
import pathlib
import re

import psycopg # PyPI: psycopg[binary]
import requests # PyPI: requests

SEEDS_DIR = pathlib.Path('/var/www/midos.house/seed')

with open('/etc/xdg/midos-house.json') as config_f:
    config = json.load(config_f)

conn = psycopg.connect('dbname=midos_house user=mido')

def b(seed_id, room=None, *, startgg=None, async_room1=None, async_room2=None):
    patch_resp = requests.get('https://ootrandomizer.com/patch/get', params={'id': seed_id})
    patch_resp.raise_for_status()
    file_stem = re.fullmatch('attachment; filename=(.*)\\.zpfz?', patch_resp.headers['Content-Disposition']).group(1)
    with open(SEEDS_DIR / re.fullmatch('attachment; filename=(.*\\.zpfz?)', patch_resp.headers['Content-Disposition']).group(1), 'wb') as patch_f:
        patch_f.write(patch_resp.content)
    api_resp = requests.get('https://ootrandomizer.com/api/v2/seed/details', params={'id': seed_id, 'key': config['ootrApiKey']})
    try:
        api_resp.raise_for_status()
    except requests.HTTPError:
        creation_timestamp = f"{datetime.datetime.strptime(input('creation timestamp: ').strip(), '%m/%d/%Y, %I:%M:%S %p UTC'):%Y-%m-%dT%H:%M:%SZ}"
        file_hash = json.loads(input('file hash: '))
    else:
        if api_resp.json()['spoilerLog'] is None:
            requests.post('https://ootrandomizer.com/api/v2/seed/unlock', params={'key': config['ootrApiKey'], 'id': seed_id}).raise_for_status()
            api_resp = requests.get('https://ootrandomizer.com/api/v2/seed/details', params={'id': seed_id, 'key': config['ootrApiKey']})
            api_resp.raise_for_status()
        with open(SEEDS_DIR / f'{file_stem}_Spoiler.json', 'w') as spoiler_f:
            spoiler_f.write(api_resp.json()['spoilerLog'])
        creation_timestamp = api_resp.json()['creationTimestamp']
        file_hash = json.loads(api_resp.json()['spoilerLog'])['file_hash']
    with conn.cursor() as cur:
        try:
            if room is not None:
                cur.execute("""UPDATE races SET
                    web_id = %s,
                    web_gen_time = %s,
                    file_stem = %s,
                    hash1 = %s,
                    hash2 = %s,
                    hash3 = %s,
                    hash4 = %s,
                    hash5 = %s
                WHERE room = %s""", (seed_id, creation_timestamp, file_stem, *file_hash, room))
            if startgg is not None:
                cur.execute("""UPDATE races SET
                    web_id = %s,
                    web_gen_time = %s,
                    file_stem = %s,
                    hash1 = %s,
                    hash2 = %s,
                    hash3 = %s,
                    hash4 = %s,
                    hash5 = %s
                WHERE startgg_set = %s""", (seed_id, creation_timestamp, file_stem, *file_hash, startgg))
            if async_room1 is not None:
                cur.execute("""UPDATE races SET
                    web_id = %s,
                    web_gen_time = %s,
                    file_stem = %s,
                    hash1 = %s,
                    hash2 = %s,
                    hash3 = %s,
                    hash4 = %s,
                    hash5 = %s
                WHERE async_room1 = %s""", (seed_id, creation_timestamp, file_stem, *file_hash, async_room1))
            if async_room2 is not None:
                cur.execute("""UPDATE races SET
                    web_id = %s,
                    web_gen_time = %s,
                    file_stem = %s,
                    hash1 = %s,
                    hash2 = %s,
                    hash3 = %s,
                    hash4 = %s,
                    hash5 = %s
                WHERE async_room2 = %s""", (seed_id, creation_timestamp, file_stem, *file_hash, async_room2))
        except Exception:
            conn.rollback()
            raise
        else:
            conn.commit()
