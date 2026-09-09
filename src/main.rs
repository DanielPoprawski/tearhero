use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const WORK_SECS: u64 = 20 * 60;
const REST_SECS: u64 = 20;
/// While a break is due and not taken, the reminder chime repeats this often.
const NAG_SECS: u64 = 180;
const CHIME_END: &str = "/usr/share/sounds/freedesktop/stereo/service-login.oga";
const CHIME_START: &str = "/usr/share/sounds/freedesktop/stereo/message-new-instant.oga";
const CHIME_NAG: &str = "/usr/share/sounds/freedesktop/stereo/window-attention.oga";

const ICON_CLOSED: &str = "\u{f0209}"; // 󰈉 nf-md-eye_off
const ICON_OUTLINE: &str = "\u{f06d0}"; // 󰛐 nf-md-eye_outline
const ICON_FILLED: &str = "\u{f0208}"; // 󰈈 nf-md-eye

enum Stored {
    /// `Some(epoch)` records when the user explicitly turned the timer off;
    /// `None` is the legacy bare `off` (or a missing file), which autostarts.
    Off(Option<u64>),
    On(u64, u64),   // (epoch seconds when work started, last check time)
    Rest(u64, u64), // (epoch seconds when rest started, last check time)
}

enum Phase {
    Off,
    Running {
        left: u64,
    },
    /// `since` is the moment the break became due; nag slots are counted from it.
    Alert {
        since: u64,
    },
    Resting {
        left: u64,
    },
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Normal,
    Silent,
}

impl Mode {
    fn as_str(self) -> &'static str {
        match self {
            Mode::Normal => "normal",
            Mode::Silent => "silent",
        }
    }
}

fn state_dir() -> PathBuf {
    env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env::var_os("HOME").expect("HOME not set")).join(".local/state")
        })
}

fn state_path() -> PathBuf {
    state_dir().join("tearhero")
}

fn mode_path() -> PathBuf {
    state_dir().join("tearhero.mode")
}

const NAG_LOCK_PREFIX: &str = "tearhero.nag.";

fn nag_lock_path(slot: u64) -> PathBuf {
    state_dir().join(format!("{NAG_LOCK_PREFIX}{slot}"))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn parse_pair(rest: &str) -> Option<(u64, u64)> {
    let parts: Vec<&str> = rest.split_whitespace().collect();
    if let [a, b] = parts[..]
        && let (Ok(start), Ok(check)) = (a.parse(), b.parse())
    {
        return Some((start, check));
    }
    // migrate old single-field format
    rest.trim().parse().ok().map(|start| (start, start))
}

/// `None` means the state could not be determined *right now* — an unreadable
/// file, or contents that do not parse. That is deliberately distinct from
/// `Some(Stored::Off(_))`: a reader that cannot tell must leave the timer alone
/// rather than announce that it is off. A missing file is the one failure that
/// really does mean off, because that is how a never-started timer looks.
fn read_stored() -> Option<Stored> {
    let contents = match fs::read_to_string(state_path()) {
        Ok(contents) => contents,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Some(Stored::Off(None)),
        Err(_) => return None,
    };
    let contents = contents.trim();
    if contents == "off" {
        return Some(Stored::Off(None));
    }
    if let Some(rest) = contents.strip_prefix("off ") {
        return rest.trim().parse().ok().map(|t| Stored::Off(Some(t)));
    }
    if let Some(rest) = contents.strip_prefix("on ") {
        return parse_pair(rest).map(|(s, c)| Stored::On(s, c));
    }
    if let Some(rest) = contents.strip_prefix("rest ") {
        return parse_pair(rest).map(|(s, c)| Stored::Rest(s, c));
    }
    // Anything else — including the empty string a half-finished write leaves
    // behind — is "cannot tell", not "off".
    None
}

/// Write-then-rename rather than a plain `fs::write`. `fs::write` opens with
/// O_TRUNC, so the file is momentarily zero bytes; a concurrent reader that
/// lands in that window reads "" and concludes the timer is off. One watcher
/// runs per monitor and they are spawned in the same instant by the shell, so
/// their 500ms loops stay phase-locked and hit that window constantly — which
/// showed up as the bar icon flickering between eye states. rename(2) is
/// atomic within a directory, so a reader sees either the old or the new
/// contents and never an empty file. The pid keeps concurrent writers from
/// clobbering each other's temp file.
fn write_atomic(path: &Path, contents: &str) {
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("tearhero");
    // `with_file_name`, not `with_extension`: the latter would map both
    // `tearhero` and `tearhero.mode` onto the same temp name.
    let tmp = path.with_file_name(format!("{name}.tmp.{}", std::process::id()));
    if fs::write(&tmp, contents).is_ok() && fs::rename(&tmp, path).is_err() {
        let _ = fs::remove_file(&tmp);
    }
}

fn write_stored(stored: &Stored) {
    let contents = match stored {
        Stored::Off(None) => "off".to_string(),
        Stored::Off(Some(t)) => format!("off {t}"),
        Stored::On(start, check) => format!("on {start} {check}"),
        Stored::Rest(start, check) => format!("rest {start} {check}"),
    };
    write_atomic(&state_path(), &contents);
}

/// Mode lives in its own file so that flipping it never races the 500ms
/// heartbeat rewrites of the state file (a read-modify-write of one shared
/// file could drop either update).
fn read_mode() -> Mode {
    match fs::read_to_string(mode_path()) {
        Ok(s) if s.trim() == "silent" => Mode::Silent,
        _ => Mode::Normal,
    }
}

fn write_mode(mode: Mode) {
    write_atomic(&mode_path(), mode.as_str());
}

const SUSPEND_THRESHOLD: u64 = 20; // fresh cycle if gap > 20 seconds

/// `None` propagates "the state is unreadable right now" from `read_stored`, so
/// callers hold their previous answer instead of falling back to `Off`.
fn phase(now: u64) -> Option<Phase> {
    let stored = read_stored()?;

    let last_check = match &stored {
        Stored::Off(_) => return Some(Phase::Off),
        Stored::On(_, check) => check,
        Stored::Rest(_, check) => check,
    };

    // A gap in the heartbeat means the system was suspended (or every watcher
    // was dead). Rather than switching off and waiting for a click, start a
    // fresh cycle: the eyes had their rest while the lid was closed.
    if now.saturating_sub(*last_check) > SUSPEND_THRESHOLD {
        write_stored(&Stored::On(now, now));
        return Some(Phase::Running { left: WORK_SECS });
    }

    let work_start = match stored {
        Stored::Off(_) => unreachable!(),
        Stored::On(t, _) => t,
        Stored::Rest(t, _) => {
            let elapsed = now.saturating_sub(t);
            if elapsed < REST_SECS {
                return Some(Phase::Resting {
                    left: REST_SECS - elapsed,
                });
            }
            t + REST_SECS
        }
    };
    let elapsed = now.saturating_sub(work_start);
    Some(if elapsed >= WORK_SECS {
        Phase::Alert {
            since: work_start + WORK_SECS,
        }
    } else {
        Phase::Running {
            left: WORK_SECS - elapsed,
        }
    })
}

/// Epoch of the current boot, from /proc/uptime.
fn boot_epoch() -> Option<u64> {
    let uptime = fs::read_to_string("/proc/uptime").ok()?;
    let secs: f64 = uptime.split_whitespace().next()?.parse().ok()?;
    Some(now_secs().saturating_sub(secs as u64))
}

/// Called once when a watcher starts. A timer that was never started, or that
/// was switched off before the current boot, starts a fresh cycle so the
/// reminder is on after login without a click. An explicit `off` from this
/// boot is respected, so `omarchy restart shell` does not undo it.
fn autostart_if_idle(now: u64) {
    match read_stored() {
        Some(Stored::Off(None)) => write_stored(&Stored::On(now, now)),
        Some(Stored::Off(Some(t))) if boot_epoch().is_some_and(|boot| t < boot) => {
            write_stored(&Stored::On(now, now))
        }
        _ => {}
    }
}

fn spawn_detached(args: &[&str]) {
    let _ = Command::new(env::current_exe().expect("current_exe"))
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

fn start_rest(now: u64) {
    write_stored(&Stored::Rest(now, now));
    // detached helper that chimes when the rest period begins and ends
    spawn_detached(&["bell", &now.to_string()]);
}

fn on(now: u64) {
    match phase(now) {
        // An unreadable state is treated as off. Atomic writes mean this can no
        // longer be a half-written file, so it is a genuinely corrupt one — and
        // the user just asked for it on, so a dead button is the worst possible
        // answer. Starting fresh also rewrites the file, healing it.
        Some(Phase::Off) | None => write_stored(&Stored::On(now, now)),
        Some(_) => {}
    }
}

fn off(now: u64) {
    write_stored(&Stored::Off(Some(now)));
}

fn toggle(now: u64) {
    match phase(now) {
        Some(Phase::Off) | None => write_stored(&Stored::On(now, now)),
        Some(Phase::Running { .. } | Phase::Resting { .. }) => off(now),
        Some(Phase::Alert { .. }) => start_rest(now),
    }
}

fn rest(now: u64) {
    match phase(now) {
        Some(Phase::Running { .. } | Phase::Alert { .. }) => start_rest(now),
        Some(Phase::Off | Phase::Resting { .. }) | None => {}
    }
}

/// Restart the work timer without taking a break. A running `bell` sees the
/// state is no longer its rest and skips the end chime.
fn skip(now: u64) {
    match phase(now) {
        Some(Phase::Off) | None => {}
        Some(_) => write_stored(&Stored::On(now, now)),
    }
}

/// `normal`/`silent` also turn the timer on when it is off, so each choice in
/// the panel is a single command. `toggle` only flips the mode.
fn set_mode(arg: Option<&str>, now: u64) -> bool {
    match arg {
        Some("normal") => {
            write_mode(Mode::Normal);
            on(now);
        }
        Some("silent") => {
            write_mode(Mode::Silent);
            on(now);
        }
        Some("toggle") => write_mode(match read_mode() {
            Mode::Normal => Mode::Silent,
            Mode::Silent => Mode::Normal,
        }),
        _ => return false,
    }
    true
}

fn play(sound: &str) {
    for player in ["pw-play", "paplay"] {
        let ok = Command::new(player)
            .arg(sound)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            return;
        }
    }
}

/// A fullscreen window on the focused Hyprland workspace. Any failure (no
/// Hyprland, no socket) counts as "not fullscreen" so chimes still play.
fn fullscreen_active() -> bool {
    Command::new("hyprctl")
        .args(["activeworkspace", "-j"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            let json: String = String::from_utf8_lossy(&o.stdout)
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect();
            json.contains("\"hasfullscreen\":true")
        })
        .unwrap_or(false)
}

/// Silent mode mutes everything; a fullscreen window mutes temporarily. Only
/// consulted by the detached chime helpers, never by the watch loop.
fn audio_allowed() -> bool {
    read_mode() != Mode::Silent && !fullscreen_active()
}

/// Runs detached for the length of one rest: chimes as the 20 seconds begin,
/// then again as they end. Both chimes live here so the pair cannot drift apart
/// and so the short-lived `toggle`/`rest` invocations never block on audio.
fn bell(rest_start: u64) {
    if audio_allowed() {
        play(CHIME_START);
    }
    let end = rest_start + REST_SECS;
    sleep(Duration::from_secs(end.saturating_sub(now_secs())));
    // skip the chime if the rest was cancelled or replaced meanwhile
    match read_stored() {
        Some(Stored::Rest(t, _)) if t == rest_start => {}
        _ => return,
    }
    // re-checked: the mode may have changed during the rest
    if audio_allowed() {
        play(CHIME_END);
    }
}

fn chime_nag() {
    if audio_allowed() {
        play(CHIME_NAG);
    }
}

/// Reminder chimes while a break is due, once per NAG_SECS slot, exactly once
/// across the N watchers (one per monitor). Each slot is claimed by creating
/// its lock file with O_EXCL; only the process that wins the create spawns the
/// chime. The claim happens before the audio gate on purpose: a slot silenced
/// by fullscreen is consumed, and the next reminder comes at the next boundary.
fn nag_tick(since: u64, now: u64, last_claimed: &mut Option<u64>) {
    let slot = since + (now.saturating_sub(since) / NAG_SECS) * NAG_SECS;
    if *last_claimed == Some(slot) {
        return;
    }
    let claimed = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(nag_lock_path(slot))
        .is_ok();
    if claimed {
        *last_claimed = Some(slot);
        spawn_detached(&["chime", "nag"]);
    }
    // On AlreadyExists another watcher owns the slot; on any other error do
    // not risk N chimes. Either way there is nothing more to do this tick.
}

/// Only locks whose slot window has fully passed are removed. Deleting locks
/// merely because this watcher does not see an alert would let a phase-locked
/// sibling that already claimed the slot lose its lock and a third tick claim
/// it again — a double chime. Slot epochs are monotonic across cycles, so an
/// expired lock can never be re-claimed and is safe to drop.
fn prune_nag_locks(now: u64) {
    let Ok(entries) = fs::read_dir(state_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(slot) = name
            .to_str()
            .and_then(|n| n.strip_prefix(NAG_LOCK_PREFIX))
            .and_then(|s| s.parse::<u64>().ok())
        else {
            continue;
        };
        if slot + NAG_SECS < now {
            let _ = fs::remove_file(entry.path());
        }
    }
}

fn json(text: &str, class: &str, tooltip: &str, mode: Mode, left: i64) -> String {
    format!(
        r#"{{"text": "{text}", "class": "{class}", "tooltip": "{}", "mode": "{}", "left": {left}}}"#,
        tooltip.replace('\n', "\\n"),
        mode.as_str()
    )
}

fn watch() -> ! {
    autostart_if_idle(now_secs());
    prune_nag_locks(now_secs());

    let mut blink = false;
    let mut last_line = String::new();
    let mut last_claimed: Option<u64> = None;
    let mut was_alert = false;
    loop {
        let now = now_secs();
        // A momentarily unreadable state means "no news": keep the last line on
        // the bar rather than flashing the off icon and back.
        let Some(current) = phase(now) else {
            sleep(Duration::from_millis(500));
            continue;
        };
        let mode = read_mode();
        let silent_note = if mode == Mode::Silent {
            "\nSilent mode"
        } else {
            ""
        };
        let line = match current {
            Phase::Off => json(
                ICON_CLOSED,
                "off",
                &format!("20-20-20 timer off\nClick for options{silent_note}"),
                mode,
                -1,
            ),
            Phase::Running { left } => json(
                ICON_OUTLINE,
                "running",
                &format!(
                    "Next eye break in {}:{:02}\nClick for options, right-click to rest now{silent_note}",
                    left / 60,
                    left % 60
                ),
                mode,
                left as i64,
            ),
            Phase::Alert { .. } => {
                let icon = if blink { ICON_FILLED } else { ICON_OUTLINE };
                json(
                    icon,
                    "alert",
                    &format!(
                        "Eye break! Right-click, then rest your eyes for 20 seconds\nA chime plays when the rest is over{silent_note}"
                    ),
                    mode,
                    -1,
                )
            }
            Phase::Resting { left } => json(
                ICON_CLOSED,
                "rest",
                &format!(
                    "Resting your eyes\u{2026} {left}s\nThe timer restarts automatically{silent_note}"
                ),
                mode,
                left as i64,
            ),
        };
        if line != last_line {
            // The reader going away (bar restart, `| head`) is the signal to stop,
            // not a reason to panic.
            let mut out = std::io::stdout().lock();
            if writeln!(out, "{line}").and_then(|_| out.flush()).is_err() {
                std::process::exit(0);
            }
            last_line = line;
        }

        match current {
            Phase::Alert { since } => {
                nag_tick(since, now, &mut last_claimed);
                was_alert = true;
            }
            _ if was_alert => {
                prune_nag_locks(now);
                last_claimed = None;
                was_alert = false;
            }
            _ => {}
        }

        // Update last_check timestamp to detect system suspension
        match read_stored() {
            Some(Stored::On(start, _)) => write_stored(&Stored::On(start, now)),
            Some(Stored::Rest(start, _)) => write_stored(&Stored::Rest(start, now)),
            Some(Stored::Off(_)) | None => {}
        }

        blink = !blink;
        sleep(Duration::from_millis(500));
    }
}

fn usage_error(msg: &str) -> ! {
    eprintln!(
        "tearhero: {msg}\nusage: tearhero [watch|on|off|toggle|rest|skip|mode <normal|silent|toggle>]"
    );
    std::process::exit(2);
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let now = now_secs();
    match args.get(1).map(String::as_str) {
        Some("on") => on(now),
        Some("off") => off(now),
        Some("toggle") => toggle(now),
        Some("rest") => rest(now),
        Some("skip") => skip(now),
        Some("mode") => {
            if !set_mode(args.get(2).map(String::as_str), now) {
                usage_error("mode expects normal, silent or toggle");
            }
        }
        Some("bell") => {
            let rest_start = args
                .get(2)
                .and_then(|t| t.parse().ok())
                .unwrap_or_else(now_secs);
            bell(rest_start);
        }
        Some("chime") => match args.get(2).map(String::as_str) {
            Some("nag") => chime_nag(),
            _ => usage_error("chime expects nag"),
        },
        Some("watch") | None => watch(),
        Some(other) => usage_error(&format!("unknown command '{other}'")),
    }
}
