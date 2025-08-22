import sys

import subprocess

subprocess.run(['cargo', 'check'], check=True)
subprocess.run(['cargo', 'sqlx', 'prepare', '--check'], check=True)
subprocess.run(['wsl', 'rsync', '--delete', '-av', '/mnt/c/Users/fenhl/git/github.com/midoshouse/midos.house/stage/', '/home/fenhl/wslgit/github.com/midoshouse/midos.house/', '--exclude', 'target'], check=True) # copy the tree to the WSL file system to improve compile times
subprocess.run(['wsl', 'rustup', 'update', 'stable'], check=True)
subprocess.run(['wsl', 'env', '-C', '/home/fenhl/wslgit/github.com/midoshouse/midos.house', 'cargo', 'check'], check=True)

with open('assets/schema.sql', encoding='utf-8') as f: #TODO check staged changes instead of worktree
    prepared_schema = f.read()
production_schema = subprocess.run(['ssh', 'midos.house', 'sudo', '-u', 'mido', 'pg_dump', '--schema-only', 'midos_house'], stdout=subprocess.PIPE, encoding='utf-8', check=True).stdout
if prepared_schema != production_schema:
    sys.exit('update assets/schema.sql (ssh midos.house sudo -u mido pg_dump --schema-only midos_house > assets/schema.sql)')
