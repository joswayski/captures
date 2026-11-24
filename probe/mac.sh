#!/bin/bash
# Each case: start "first" app, then "second", press ⌘⇧Space, report who got it.
set -u
cd "$(dirname "$0")"
swiftc -O hotkey.swift -o /tmp/hotkey
swiftc -O press.swift -o /tmp/press
run_case() { # label first_excl second_excl (second may be "-" for none)
  echo "=== $1"
  /tmp/hotkey first "$2" 6 & a=$!
  sleep 1.5
  if [ "$3" != "-" ]; then /tmp/hotkey second "$3" 4.5 & b=$!; sleep 1.5; fi
  /tmp/press
  osascript -e 'tell application "System Events" to keystroke space using {command down, shift down}' 2>&1 | sed 's/^/osascript: /'
  wait
}
run_case "control: one app alone" 0 -
run_case "both non-exclusive (1Password-like holder first, Captures second)" 0 0
run_case "exclusive holder first, Captures non-exclusive" 1 0
run_case "non-exclusive holder first, Captures tries exclusive" 0 1
