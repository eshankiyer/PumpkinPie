#!/bin/bash
# Idempotent: starts the Pumpkin differential server if it is not already answering on RCON.
cd "$(dirname "$0")"
exec 8>.start.lock; flock 8
if (exec 3<>/dev/tcp/127.0.0.1/25577) 2>/dev/null; then echo "already up"; exit 0; fi
[ -f pid ] && kill -0 "$(cat pid)" 2>/dev/null && { echo "process alive but RCON down; see server.log"; exit 1; }
[ -f server.log ] && mv server.log "server.log.$(date +%s)"
nohup /home/eshanki/pumpkin-wt/behav/target/debug/pumpkin > server.log 2>&1 &
echo $! > pid
for i in $(seq 1 120); do (exec 3<>/dev/tcp/127.0.0.1/25577) 2>/dev/null && { echo "up pid $(cat pid)"; exit 0; }; kill -0 "$(cat pid)" 2>/dev/null || { echo "died; tail:"; tail -20 server.log; exit 1; }; sleep 1; done
echo "timeout"; exit 1
