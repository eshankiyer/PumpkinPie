#!/bin/bash
cd /home/eshanki/Pumpkin || exit 1
export VANILLA_DECOMPILE=~/pumpkin-vanilla-26.2/decompiled
python3 conformance/method_gaps.py --workspace /home/eshanki/pumpkin-wt/staging > /home/eshanki/pumpkin-campaign/method_gaps_new.json 2> /home/eshanki/pumpkin-campaign/regen.err
echo "REGEN rc=$? bytes=$(stat -c%s /home/eshanki/pumpkin-campaign/method_gaps_new.json 2>/dev/null)"
