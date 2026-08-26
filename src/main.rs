use std::env;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const WORK_SECS: u64 = 20 * 60;
const REST_SECS: u64 = 20;
const CHIME_END: &str = "/usr/share/sounds/freedesktop/stereo/service-login.oga";
const CHIME_START: &str = "/usr/share/sounds/freedesktop/stereo/message-new-instant.oga";

const ICON_CLOSED: &str = "\u{f0209}"; // 󰈉 nf-md-eye_off
const ICON_OUTLINE: &str = "\u{f06d0}"; // 󰛐 nf-md-eye_outline
const ICON_FILLED: &str = "\u{f0208}"; // 󰈈 nf-md-eye

enum Stored {
    Off,
    On(u64, u64),   // (epoch seconds when work started, last check time)
    Rest(u64, u64), // (epoch seconds when rest started, last check time)
}

enum Phase {
    Off,
    Running { left: u64 },
    Alert,
    Resting { left: u64 },
}

fn state_path() -> PathBuf {
    env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env::var_os("HOME").expect("HOME not set")).join(".local/state")
        })
        .join("tearhero")
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// `None` means the state could not be determined *right now* — an unreadable
/// file, or contents that do not parse. That is deliberately distinct from
/// `Some(Stored::Off)`: a reader that cannot tell must leave the timer alone
/// rather than announce that it is off. A missing file is the one failure that
/// really does mean off, because that is how a never-started timer looks.
fn read_stored() -> Option<Stored> {
    let contents = match fs::read_to_string(state_path()) {
        Ok(contents) => contents,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Some(Stored::Off),
        Err(_) => return None,
    };
    let contents = contents.trim();
    if contents == "off" {
        return Some(Stored::Off);
    }
    if let Some(rest) = contents.strip_prefix("on ") {
        let parts: Vec<&str> = rest.split_whitespace().collect();
        if parts.len() == 2 {
            if let (Ok(start), Ok(check)) = (parts[0].parse(), parts[1].parse()) {
                return Some(Stored::On(start, check));
            }
        }
        if let Ok(start) = rest.parse() {
            return Some(Stored::On(start, start)); // migrate old format
        }
    } else if let Some(rest) = contents.strip_prefix("rest ") {
        let parts: Vec<&str> = rest.split_whitespace().collect();
        if parts.len() == 2 {
            if let (Ok(start), Ok(check)) = (parts[0].parse(), parts[1].parse()) {
                return Some(Stored::Rest(start, check));
            }
        }
        if let Ok(start) = rest.parse() {
            return Some(Stored::Rest(start, start)); // migrate old format
        }
    }
    // Anything else — including the empty string a half-finished write leaves
    // behind — is "cannot tell", not "off".
    None
}

fn write_stored(stored: &Stored) {
    let path = state_path();
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let contents = match stored {
        Stored::Off => "off".to_string(),
        Stored::On(start, check) => format!("on {start} {check}"),
        Stored::Rest(start, check) => format!("rest {start} {check}"),
    };
    // Write-then-rename rather than a plain `fs::write`. `fs::write` opens with
    // O_TRUNC, so the file is momentarily zero bytes; a concurrent reader that
    // lands in that window reads "" and concludes the timer is off. One watcher
    // runs per monitor and they are spawned in the same instant by the shell, so
    // their 500ms loops stay phase-locked and hit that window constantly — which
    // showed up as the bar icon flickering between eye states. rename(2) is
    // atomic within a directory, so a reader sees either the old or the new
    // contents and never an empty file. The pid keeps concurrent writers from
    // clobbering each other's temp file.
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    if fs::write(&tmp, contents).is_ok() && fs::rename(&tmp, &path).is_err() {
        let _ = fs::remove_file(&tmp);
    }
}

const SUSPEND_THRESHOLD: u64 = 20; // reset if gap > 20 seconds

/// `None` propagates "the state is unreadable right now" from `read_stored`, so
/// callers hold their previous answer instead of falling back to `Off`.
fn phase(now: u64) -> Option<Phase> {
    let stored = read_stored()?;

    // Check if system was suspended (time gap > 20 seconds)
    let last_check = match &stored {
        Stored::Off => return Some(Phase::Off),
        Stored::On(_, check) => check,
        Stored::Rest(_, check) => check,
    };

    if now.saturating_sub(*last_check) > SUSPEND_THRESHOLD {
        write_stored(&Stored::Off);
        return Some(Phase::Off);
    }

    let work_start = match stored {
        Stored::Off => unreachable!(),
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
        Phase::Alert
    } else {
        Phase::Running {
            left: WORK_SECS - elapsed,
        }
    })
}

fn start_rest(now: u64) {
    write_stored(&Stored::Rest(now, now));
    // detached helper that chimes when the rest period ends
    let _ = Command::new(env::current_exe().expect("current_exe"))
        .arg("bell")
        .arg(now.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

fn toggle() {
    let now = now_secs();
    match phase(now) {
        // An unreadable state is treated as off. Atomic writes mean this can no
        // longer be a half-written file, so it is a genuinely corrupt one — and
        // the user just clicked the icon, so a dead button is the worst possible
        // answer. Starting fresh also rewrites the file, healing it.
        Some(Phase::Off) | None => write_stored(&Stored::On(now, now)),
        Some(Phase::Running { .. } | Phase::Resting { .. }) => write_stored(&Stored::Off),
        Some(Phase::Alert) => start_rest(now),
    }
}

fn rest() {
    let now = now_secs();
    match phase(now) {
        Some(Phase::Running { .. } | Phase::Alert) => start_rest(now),
        Some(Phase::Off | Phase::Resting { .. }) | None => {}
    }
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

/// Runs detached for the length of one rest: chimes as the 20 seconds begin,
/// then again as they end. Both chimes live here so the pair cannot drift apart
/// and so the short-lived `toggle`/`rest` invocations never block on audio.
fn bell(rest_start: u64) {
    play(CHIME_START);
    let end = rest_start + REST_SECS;
    sleep(Duration::from_secs(end.saturating_sub(now_secs())));
    // skip the chime if the rest was cancelled or replaced meanwhile
    match read_stored() {
        Some(Stored::Rest(t, _)) if t == rest_start => {}
        _ => return,
    }
    play(CHIME_END);
}

fn json(text: &str, class: &str, tooltip: &str) -> String {
    format!(
        r#"{{"text": "{text}", "class": "{class}", "tooltip": "{}"}}"#,
        tooltip.replace('\n', "\\n")
    )
}

fn watch() -> ! {
    let mut blink = false;
    let mut last_line = String::new();
    loop {
        let now = now_secs();
        // A momentarily unreadable state means "no news": keep the last line on
        // the bar rather than flashing the off icon and back.
        let Some(current) = phase(now) else {
            sleep(Duration::from_millis(500));
            continue;
        };
        let line = match current {
            Phase::Off => json(ICON_CLOSED, "off", "20-20-20 timer off\nClick to start"),
            Phase::Running { left } => json(
                ICON_OUTLINE,
                "running",
                &format!(
                    "Next eye break in {}:{:02}\nClick to turn off, right-click to rest now",
                    left / 60,
                    left % 60
                ),
            ),
            Phase::Alert => {
                let icon = if blink { ICON_FILLED } else { ICON_OUTLINE };
                json(
                    icon,
                    "alert",
                    "Eye break! Click, then rest your eyes for 20 seconds\nA chime plays when the rest is over",
                )
            }
            Phase::Resting { left } => json(
                ICON_CLOSED,
                "rest",
                &format!("Resting your eyes\u{2026} {left}s\nThe timer restarts automatically"),
            ),
        };
        if line != last_line {
            println!("{line}");
            std::io::stdout().flush().ok();
            last_line = line;
        }

        // Update last_check timestamp to detect system suspension
        match read_stored() {
            Some(Stored::On(start, _)) => write_stored(&Stored::On(start, now)),
            Some(Stored::Rest(start, _)) => write_stored(&Stored::Rest(start, now)),
            Some(Stored::Off) | None => {}
        }

        blink = !blink;
        sleep(Duration::from_millis(500));
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("toggle") => toggle(),
        Some("rest") => rest(),
        Some("bell") => {
            let rest_start = args
                .get(2)
                .and_then(|t| t.parse().ok())
                .unwrap_or_else(now_secs);
            bell(rest_start);
        }
        Some("watch") | None => watch(),
        Some(other) => {
            eprintln!("tearhero: unknown command '{other}' (expected: watch, toggle, rest)");
            std::process::exit(2);
        }
    }
}
