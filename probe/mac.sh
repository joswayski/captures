#!/bin/bash
set -u
cd "$(dirname "$0")"
swiftc -O hotkey.swift -o /tmp/hotkey
swiftc -O press.swift -o /tmp/press
run_case() { # label first_excl second_excl
  echo "=== $1"
  /tmp/hotkey first "$2" 6 & a=$!
  sleep 1.5
  if [ "$3" != "-" ]; then /tmp/hotkey second "$3" 4.5 & b=$!; sleep 1.5; fi
  /tmp/press
  wait
}
run_case "exclusive vs exclusive" 1 1
run_case "Captures first, exclusive app second" 0 1
echo "=== real 1Password"
brew install --cask 1password >/dev/null 2>&1; ls -d /Applications/1Password.app
xattr -dr com.apple.quarantine /Applications/1Password.app
open -a /Applications/1Password.app; sleep 40
pgrep -fl 1Password | head -5
defaults read com.1password.1password 2>/dev/null | head -20
for excl in 0 1; do
  /tmp/hotkey captures "$excl" 4 & sleep 1.5; /tmp/press; wait
  osascript -e 'tell application "System Events" to get name of every process whose visible is true' 2>&1
  osascript -e 'tell application "System Events" to get name of every window of process "1Password"' 2>&1
done
screencapture -x /tmp/after.png; echo shot=$?
