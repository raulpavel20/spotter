#!/usr/bin/env bash
# Records assets/demo.gif with vhs (https://github.com/charmbracelet/vhs).
# Needs vhs, ttyd, ffmpeg and perl, and gifsicle to shrink the result.
# Builds a release binary and the demo project (demo-project.sh), then
# records a review session. Run from anywhere:
#   assets/record-demo.sh
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
cargo build --release --manifest-path "$ROOT/Cargo.toml" -q
"$ROOT/assets/demo-project.sh" "$WORK/billing-api" >/dev/null
mkdir -p "$WORK/home"

# The agent, in the background: once the first file is marked viewed, it
# commits its work in progress, and the new commit shows up live.
cat > "$WORK/agent.sh" <<'EOF'
cd "$1"
cp .git/spotter/viewed.json ../marks.before
while cmp -s .git/spotter/viewed.json ../marks.before; do sleep 0.1; done
sleep 3
git add -A
git commit -qm "Make POST /pay idempotent"
EOF

# --- tape helpers: keys pressed the way a person presses them -------------

# Keys type instantly (TypingSpeed 0); the Sleeps set the rhythm.
# tap KEY N MS: N presses roughly MS apart, never quite evenly.
JITTER=(0 25 -15 40 -10 20 -25 10)
J=0
tap() {
  local i
  for ((i = 0; i < $2; i++)); do
    echo "$1"
    if ((i < $2 - 1)); then
      echo "Sleep $(($3 + JITTER[J++ % ${#JITTER[@]}]))ms"
    fi
  done
}
# hold KEY N: a held-down key, at the keyboard's repeat rate.
hold() { echo "$1@28ms $2"; }
pause() { echo "Sleep $1"; }

# --- the recording -------------------------------------------------------

{
cat <<EOF
Output demo.gif
Set Shell "bash"
Set FontSize 14
Set Width 1320
Set Height 660
Set Padding 14
Set Framerate 30
Set Theme "Catppuccin Mocha"
Set TypingSpeed 0
Set WindowBar Colorful
Set BorderRadius 8
Env COLORTERM "truecolor"
Env GIT_CONFIG_GLOBAL "/dev/null"
Env HOME "$WORK/home"
Env XDG_CONFIG_HOME ""

Hide
Type "export PS1='\$ ' PATH=$ROOT/target/release:\$PATH && cd $WORK/billing-api"
Enter
Type "(bash $WORK/agent.sh \$PWD > /dev/null 2>&1 &) && clear"
Enter
Sleep 300ms
Show

Sleep 700ms
Type@80ms "spotter"
Sleep 300ms
Enter
Sleep 1.8s
EOF

# Wander down the timeline, overshoot, come back.
tap Down 3 130; pause 450ms
tap Down 3 110; pause 300ms
tap Down 2 150; pause 650ms
tap Up 2 140; pause 800ms
# Into the commit's files, look around, open one.
echo Enter; pause 650ms
tap Down 2 140; pause 350ms
tap Down 3 120; pause 550ms
echo Up; pause 750ms
echo Enter; pause 1.4s
# Read down the diff.
hold Down 14; pause 700ms
hold Down 8; pause 1.1s
# Jump around with the explorer.
echo Left; pause 550ms
echo Up; pause 650ms
echo Up; pause 500ms
echo Up; pause 1s
# Back in the diff: skim, scroll back, mark the file viewed.
echo Right; pause 450ms
hold Down 12; pause 600ms
hold Up 5; pause 500ms
hold Down 10; pause 900ms
echo Space; pause 1.5s
hold Down 6; pause 900ms
# Back out; the agent commits, and its commit shows up at the top.
echo Escape; pause 3.4s
echo Left; pause 350ms
tap Up 6 100; pause 2.8s
} > "$WORK/demo.tape"

(cd "$WORK" && vhs demo.tape)
if command -v gifsicle >/dev/null; then
  gifsicle -O3 --lossy=80 "$WORK/demo.gif" -o "$ROOT/assets/demo.gif"
else
  echo "gifsicle not found: the GIF is not optimized" >&2
  cp "$WORK/demo.gif" "$ROOT/assets/demo.gif"
fi
echo "wrote $ROOT/assets/demo.gif"
