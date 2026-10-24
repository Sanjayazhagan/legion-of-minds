#!/bin/bash
# Records one scripted tour of the real app to an MP4 (needs ffmpeg).
# usage: tools/record.sh tools/tours/attack.txt videos/attack.mp4 [save_dir]
# On a headless Linux box, start a display first: Xvfb :99 -screen 0 1280x800x24 & export DISPLAY=:99
set -e
tour=$1; out=$2; save=${3:-}
[ -n "$save" ] && export BAG_SAVE_DIR=$save
BAG_TOUR=$tour BAG_RECORD=$out cargo run --release -p legion-of-minds
