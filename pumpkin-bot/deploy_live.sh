#!/bin/bash
# Wait for the crit test to finish, then build and permanently deploy the
# all-fixes server on the NUC.
N="eshan@192.168.1.93"
for i in $(seq 1 120); do
  grep -q RESULT /tmp/crit.txt 2>/dev/null && break
  sleep 30
done
echo "crit test done: $(grep RESULT /tmp/crit.txt 2>/dev/null)"

ssh $N 'cd ~/pumpkin-dev/Pumpkin && git checkout -q live && docker rm -f pumpkin-builder 2>/dev/null >/dev/null
docker run -d --name pumpkin-builder -v /var/run/docker.sock:/var/run/docker.sock -v /home/eshan/pumpkin-dev/Pumpkin:/src -w /src docker:cli docker build -t pumpkin:live . >/dev/null && echo LIVE_BUILD_STARTED'

for i in $(seq 1 120); do
  ready=$(ssh -o ConnectTimeout=8 $N "docker images pumpkin:live --format x 2>/dev/null" 2>/dev/null)
  [ "$ready" = "x" ] && break
  sleep 60
done
echo "live image built"

# Permanent server: own world dir, restarts on boot and on crash.
ssh $N 'mkdir -p /home/eshan/pumpkin-live
docker rm -f pumpkin-test pumpkin-live 2>/dev/null >/dev/null
if [ ! -f /home/eshan/pumpkin-live/pumpkin.toml ] && [ -f /home/eshan/pumpkin-dev/server/pumpkin.toml ]; then
  cp /home/eshan/pumpkin-dev/server/pumpkin.toml /home/eshan/pumpkin-live/pumpkin.toml
fi
docker run -d --name pumpkin-live --restart unless-stopped -i \
  -p 25565:25565 -p 25575:25575 \
  -v /home/eshan/pumpkin-live:/pumpkin -u 1000 pumpkin:live >/dev/null
sleep 12
docker ps --filter name=pumpkin-live --format "{{.Image}} {{.Status}}"
docker logs pumpkin-live 2>&1 | grep -o "Server is now running.*" | head -1'
echo DEPLOY_DONE
