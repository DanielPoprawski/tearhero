# tearhero

vibe coded project to save my eyeballs from sahara desert levels of dryness

A 20-20-20 eye-break reminder for Omarchy: every 20 minutes, look at something
20 feet away for 20 seconds. A tiny zero-dependency Rust binary owns the timer;
an Omarchy shell plugin draws it in the bar and gives you a popup panel.

## Install

```sh
./install.sh
```

Builds the release binary into `~/.local/bin/tearhero`, installs the plugin to
`~/.config/omarchy/plugins/tearhero/`, migrates the old bar module entry in
`~/.config/omarchy/shell.json`, and restarts the shell. The timer starts by
itself once the bar is up.

## Using it

Bar icon:

- **Left-click** opens the panel (status, countdown, mode, actions).
- **Right-click** starts the 20 second rest right away.
- **Middle-click** skips the current cycle and restarts the 20 minute timer.

Modes (chosen in the panel):

- **Normal** — when a break is due, a chime plays immediately and then every
  3 minutes until you rest or skip. The rest itself has a start and end chime.
- **Silent** — identical timer and visuals, no audio at all.
- **Off** — timer stopped. Stays off across `omarchy restart shell`, but a
  reboot turns the timer back on.

A fullscreen window (games, video) temporarily behaves like silent mode: no
chimes, and the due-break alert is dimmed until you leave fullscreen. The timer
keeps running underneath.

After a laptop suspend the timer starts a fresh cycle instead of turning off.

## CLI

```
tearhero watch                     stream bar JSON (one line per change)
tearhero on | off | toggle         start / stop the timer
tearhero rest                      start the 20 s rest now
tearhero skip                      restart the 20 min cycle without resting
tearhero mode normal|silent|toggle change mode (normal/silent also turn it on)
```

IPC from the shell: `omarchy-shell ipc call tearhero toggle|open|close|rest|skip`.

## Files

- `~/.local/state/tearhero` — timer state (`off [epoch]` | `on <start> <check>` | `rest <start> <check>`)
- `~/.local/state/tearhero.mode` — `normal` or `silent`
- `~/.local/state/tearhero.nag.<epoch>` — short-lived lock files that make each
  reminder chime play once even with one watcher per monitor
