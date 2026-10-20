#!/usr/bin/env bash
# Provision a real vanilla Minecraft server on the NUC as a behavioural oracle
# next to Pumpkin.
#
# The point: "what does vanilla do here?" is currently answered by reading
# sources, which is slow and has twice produced claims that had to be retracted
# publicly. With a real vanilla server of the same version running alongside,
# the question becomes an experiment that answers itself, and the coding agent
# can take its target from a measurement instead of from somebody's memory.
#
# Runs in Docker because the NUC has no JRE and no passwordless sudo, but does
# have Docker, which is also how Pumpkin runs there.
#
# Idempotent. Safe to re-run.
set -euo pipefail

NUC="${NUC:-eshan@192.168.1.93}"
VERSION="${VERSION:-26.2}"
DIR="${DIR:-/home/eshan/vanilla-oracle}"
NAME="${NAME:-vanilla-oracle}"
RCON_PORT="${RCON_PORT:-25576}"   # Pumpkin holds 25575
GAME_PORT="${GAME_PORT:-25566}"   # Pumpkin holds 25565
RCON_PASS="${RCON_PASS:-oracletest}"
SEED="${SEED:-1784236824834363978}"   # same seed as the Pumpkin test world
JRE_IMAGE="${JRE_IMAGE:-eclipse-temurin:25-jre}"

say() { printf '\n[oracle] %s\n' "$*"; }

say "resolving $VERSION from Mojang's version manifest"
# Two curl hops rather than one curl plus python urllib: urllib failed DNS
# resolution when this script ran detached, while curl was fine.
META_URL="$(
  curl -sS -m 30 https://launchermeta.mojang.com/mc/game/version_manifest_v2.json |
    python3 -c "
import json,sys
d=json.load(sys.stdin)
m=[v['url'] for v in d['versions'] if v['id']=='$VERSION']
print(m[0] if m else '')
"
)"
[ -n "$META_URL" ] || { echo "  version $VERSION not in manifest" >&2; exit 1; }

read -r JAR_URL JAR_SHA <<<"$(
  curl -sS -m 30 "$META_URL" |
    python3 -c "
import json,sys
s=json.load(sys.stdin)['downloads']['server']
print(s['url'], s['sha1'])
"
)"
if [ -z "$JAR_URL" ] || [ -z "$JAR_SHA" ]; then
  echo "  could not resolve server.jar for $VERSION" >&2
  exit 1
fi
echo "  sha1 $JAR_SHA"

say "provisioning $DIR on the NUC"
ssh "$NUC" "mkdir -p '$DIR'"

say "downloading server.jar, skipped when the sha1 already matches"
ssh "$NUC" "
  cd '$DIR'
  if [ -f server.jar ] && [ \"\$(sha1sum server.jar | cut -d' ' -f1)\" = '$JAR_SHA' ]; then
    echo '  already present, sha1 verified'
  else
    curl -sSL -o server.jar '$JAR_URL'
    got=\$(sha1sum server.jar | cut -d' ' -f1)
    [ \"\$got\" = '$JAR_SHA' ] || { echo \"  sha1 mismatch: \$got\" >&2; exit 1; }
    echo '  downloaded, sha1 verified'
  fi
"

say "writing server.properties"
ssh "$NUC" "cat > '$DIR/server.properties'" <<EOF
server-port=$GAME_PORT
enable-rcon=true
rcon.port=$RCON_PORT
rcon.password=$RCON_PASS
broadcast-rcon-to-ops=false
online-mode=false
level-seed=$SEED
level-name=world
gamemode=creative
difficulty=normal
spawn-protection=0
max-players=5
view-distance=10
simulation-distance=10
enable-command-block=true
motd=Pumpkin differential oracle
EOF

say "EULA"
if ssh "$NUC" "grep -q 'eula=true' '$DIR/eula.txt' 2>/dev/null"; then
  echo "  already accepted"
else
  echo "  NOT accepted."
  echo "  Mojang requires agreement to https://aka.ms/MinecraftEULA before the"
  echo "  server will start. That is the account owner's decision, so this script"
  echo "  does not set it. To accept and then continue:"
  echo "    ssh $NUC \"echo eula=true > $DIR/eula.txt\""
  echo "    bash $0"
  exit 0
fi

say "pulling $JRE_IMAGE"
ssh "$NUC" "docker pull -q '$JRE_IMAGE' >/dev/null && echo '  ok'"

say "starting container $NAME"
ssh "$NUC" "
  docker rm -f '$NAME' >/dev/null 2>&1 || true
  docker run -d --name '$NAME' --restart unless-stopped \
    -p $GAME_PORT:$GAME_PORT -p $RCON_PORT:$RCON_PORT \
    -v '$DIR':/data -w /data '$JRE_IMAGE' \
    java -Xms1G -Xmx2G -jar server.jar nogui >/dev/null
  echo '  container started'
"

say "waiting for the world to generate, this takes a minute on first run"
for i in $(seq 1 40); do
  if ssh "$NUC" "docker logs '$NAME' 2>&1 | grep -q 'RCON running\|Done ('" ; then
    echo "  up"
    break
  fi
  sleep 10
done

ssh "$NUC" "docker logs '$NAME' 2>&1 | tail -4" | sed 's/^/  /'
say "ready. Compare with: python3 differential.py run random_tick_rate"
