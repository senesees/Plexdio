#!/usr/bin/env bash
# Regenerate the small audio fixtures used by the mock Plex server and decoder tests.
# Needs an ffmpeg with libmp3lame/libvorbis/libopus (e.g. a static build); the outputs are
# committed so normal builds never need ffmpeg.   Usage: tools/gen_fixtures.sh /path/to/ffmpeg
set -euo pipefail
FF=${1:-ffmpeg}
OUT=$(dirname "$0")/../crates/mockplex/fixtures/audio
mkdir -p "$OUT"
q() { "$FF" -hide_banner -loglevel error -y "$@"; }
# A little arpeggio per file so tracks are distinguishable by ear.
mel() { # $1 base freq, $2 amplitude
  echo "aevalsrc=exprs='$2*sin(2*PI*$1*pow(1.25992,mod(floor(t*4),4))*t)|$2*sin(2*PI*$1*pow(1.25992,mod(floor(t*4),4))*t*1.002)':s=44100:d=8"
}
q -f lavfi -i "$(mel 220 0.5)" -c:a libmp3lame -b:a 128k "$OUT/t01.mp3"
q -f lavfi -i "$(mel 247 0.5)" -c:a flac "$OUT/t02.flac"
q -f lavfi -i "$(mel 262 0.5)" -c:a aac -b:a 128k "$OUT/t03.m4a"
q -f lavfi -i "$(mel 294 0.5)" -c:a libvorbis -q:a 3 "$OUT/t04.ogg"
q -f lavfi -i "$(mel 330 0.5)" -c:a libopus -b:a 64k "$OUT/t05.opus"
q -f lavfi -i "$(mel 349 0.5)" -ac 1 -ar 22050 -c:a pcm_s16le "$OUT/t06.wav"
q -f lavfi -i "$(mel 392 0.5)" -c:a alac "$OUT/t07_alac.m4a"
# Loud and quiet tracks for normalization (with ReplayGain tags on the flac).
q -f lavfi -i "$(mel 196 0.95)" -c:a flac -metadata REPLAYGAIN_TRACK_GAIN="-8.50 dB" -metadata REPLAYGAIN_TRACK_PEAK="0.950000" "$OUT/t08_loud.flac"
q -f lavfi -i "$(mel 440 0.08)" -c:a libmp3lame -b:a 128k "$OUT/t09_quiet.mp3"
# Gapless pair: one continuous 330 Hz sine split sample-accurately into two FLAC files.
q -f lavfi -i "sine=frequency=330:sample_rate=44100:duration=6" -af "atrim=end_sample=132300" -c:a flac "$OUT/gapless_a.flac"
q -f lavfi -i "sine=frequency=330:sample_rate=44100:duration=6" -af "atrim=start_sample=132300,asetpts=PTS-STARTPTS" -c:a flac "$OUT/gapless_b.flac"
ls -l "$OUT"
