//! Bracketed paste must be off while a child command runs, otherwise programs
//! that don't understand mode 2004 (nano/pico, cat, ...) receive `ESC[200~`.

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if from >= hay.len() {
        return None;
    }
    hay[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

#[test]
fn bracketed_paste_disabled_before_command_runs() {
    let pair = native_pty_system()
        .openpty(PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 })
        .expect("openpty");
    let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_itsadri-terminal"));
    cmd.arg("--shell");
    cmd.env_remove("ITSADRI_TERMINAL");
    // Isolate history/config/import files from the real user profile.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let home = std::env::temp_dir().join(format!("itsadri-bp-{}-{}", std::process::id(), nanos));
    std::fs::create_dir_all(&home).expect("temp home");
    cmd.env("HOME", &home);
    cmd.env("XDG_DATA_HOME", &home);
    cmd.env("XDG_CONFIG_HOME", &home);
    let mut child = pair.slave.spawn_command(cmd).expect("spawn");
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().expect("reader");
    let mut writer = pair.master.take_writer().expect("writer");
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut b = [0u8; 4096];
        while let Ok(n) = reader.read(&mut b) {
            if n == 0 || tx.send(b[..n].to_vec()).is_err() {
                break;
            }
        }
    });

    let on = b"\x1b[?2004h";
    let off = b"\x1b[?2004l";
    let mut out: Vec<u8> = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(15);
    let pump = |out: &mut Vec<u8>, until: &dyn Fn(&[u8]) -> bool| -> bool {
        while Instant::now() < deadline {
            if until(out) {
                return true;
            }
            if let Ok(chunk) = rx.recv_timeout(Duration::from_millis(100)) {
                out.extend_from_slice(&chunk);
            }
        }
        until(out)
    };

    // Wait for the prompt to enable paste mode and finish rendering, then run a command.
    let prompt = "❯".as_bytes();
    assert!(
        pump(&mut out, &|o| {
            find(o, on, 0).is_some_and(|p| find(o, prompt, p).is_some())
        }),
        "no prompt"
    );
    let cmd_start = out.len();
    writer.write_all(b"echo MARK\"ER_OK\"\r").unwrap();
    writer.flush().unwrap();
    // Output line (not the echoed input, which contains MARK"ER_OK").
    let ok = pump(&mut out, &|o| find(o, b"MARKER_OK", cmd_start).is_some());
    let marker = find(&out, b"MARKER_OK", cmd_start);
    let _ = writer.write_all(b"exit\r");
    let _ = writer.flush();
    let exit_deadline = Instant::now() + Duration::from_secs(5);
    let mut exited = false;
    while Instant::now() < exit_deadline {
        if let Ok(Some(_)) = child.try_wait() {
            exited = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    if !exited {
        let _ = child.kill();
        let _ = child.wait();
    }
    let _ = std::fs::remove_dir_all(&home);
    assert!(ok, "command output never appeared: {:?}", String::from_utf8_lossy(&out));

    let marker = marker.unwrap();
    let off_pos = find(&out[..marker], off, cmd_start);
    assert!(
        off_pos.is_some(),
        "mode 2004 not disabled before command output: {:?}",
        String::from_utf8_lossy(&out)
    );
}
