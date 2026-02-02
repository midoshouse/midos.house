import sys

import re
import subprocess

subprocess.run(['cargo', 'check'], check=True)
subprocess.run(['wsl', '-d', 'ubuntu-m2', 'sudo', '-n', 'apt-get', 'install', '-y', 'pkg-config', 'libssl-dev'], check=True)
subprocess.run(['wsl', '-d', 'ubuntu-m2', '/home/fenhl/.cargo/bin/rustup', 'update', 'stable'], check=True)
subprocess.run(['wsl', '-d', 'ubuntu-m2', '/home/fenhl/.cargo/bin/cargo', 'install', 'sqlx-cli'], check=True)
subprocess.run(['wsl', '-d', 'ubuntu-m2', 'rsync', '--mkpath', '--delete', '-av', '/mnt/c/Users/fenhl/git/github.com/midoshouse/midos.house/stage/', '/home/fenhl/wslgit/github.com/midoshouse/midos.house/', '--exclude', 'target'], check=True) # copy the tree to the WSL file system to improve compile times
subprocess.run(['wsl', '-d', 'ubuntu-m2', 'env', '-C', '/home/fenhl/wslgit/github.com/midoshouse/midos.house', '/home/fenhl/.cargo/bin/cargo', 'check'], check=True)
if subprocess.run(['wsl', '-d', 'ubuntu-m2', 'env', '-C', '/home/fenhl/wslgit/github.com/midoshouse/midos.house', '/home/fenhl/.cargo/bin/cargo', 'sqlx', 'prepare', '--check']).returncode != 0:
    sys.exit('update .sqlx (wsl -d ubuntu-m2 /home/fenhl/.cargo/bin/cargo sqlx prepare)')

prepared_schema = re.sub(r'\\(un)?restrict \w*', r'\\\1restrict NSkHPci93sAFqHtSzSNGsBd7dCxhH7NpHe4WhC8jFzIipftC7A6hpgap0hCfbqM', subprocess.run(['git', 'show', ':assets/schema.sql'], stdout=subprocess.PIPE, encoding='utf-8', check=True).stdout)
production_schema = re.sub(r'\\(un)?restrict \w*', r'\\\1restrict NSkHPci93sAFqHtSzSNGsBd7dCxhH7NpHe4WhC8jFzIipftC7A6hpgap0hCfbqM', subprocess.run(['ssh', 'midos.house', 'sudo -u mido pg_dump --schema-only midos_house'], stdout=subprocess.PIPE, encoding='utf-8', check=True).stdout)
if prepared_schema != production_schema:
    sys.exit(r'''update assets/schema.sql (ssh midos.house 'sudo -u mido pg_dump --schema-only midos_house | sed -e "s/\\\\restrict [[:alnum:]]*/\\\\restrict NSkHPci93sAFqHtSzSNGsBd7dCxhH7NpHe4WhC8jFzIipftC7A6hpgap0hCfbqM/g" | sed -e "s/\\\\unrestrict [[:alnum:]]*/\\\\unrestrict NSkHPci93sAFqHtSzSNGsBd7dCxhH7NpHe4WhC8jFzIipftC7A6hpgap0hCfbqM/g"' > assets/schema.sql)''')
