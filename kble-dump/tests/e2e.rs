//! End-to-end tests for the `kble-dump` record/replay plug.
//!
//! `kble-dump record <dir>` writes each frame it receives to a dump file and
//! echoes it back; `kble-dump replay <file>` plays a dump file's frames back
//! out (honoring the recorded inter-frame timing). The test drives both as real
//! processes over the WebSocket-over-stdio protocol via
//! [`kble_test_support::Plug`], recording a handful of frames and then replaying
//! them.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use bytes::Bytes;
use kble_test_support::Plug;
use tokio::process::Command;

/// Build a `Command` running the freshly-built `kble-dump` binary.
fn kble_dump(args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_kble-dump"));
    cmd.args(args);
    cmd
}

/// A fresh, uniquely-named directory under the per-test-binary temp dir Cargo
/// provides.
fn unique_tmp_dir(prefix: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    // `CARGO_TARGET_TMPDIR` persists across test runs and the dir is never
    // cleaned, so a pid (which the OS can recycle) + counter name could collide
    // with a stale dir from an earlier run. Start each one empty so
    // `find_dump_file` sees only this run's dump.
    let dir =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{prefix}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// The single `*.bin` dump file `record` wrote into `dir`.
fn find_dump_file(dir: &Path) -> PathBuf {
    let mut bins: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("read dump dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "bin"))
        .collect();
    assert_eq!(
        bins.len(),
        1,
        "expected exactly one dump file, got {bins:?}"
    );
    bins.pop().unwrap()
}

/// Record `frames` to a fresh dump file and return its path. `record` echoes
/// each frame back, so we also assert the echo here; closing the input ends the
/// recording and the process must exit cleanly.
async fn record_dump(frames: &[&[u8]]) -> PathBuf {
    let dir = unique_tmp_dir("kble-dump-rec");
    let mut plug = Plug::spawn(kble_dump(&["record", dir.to_str().unwrap()]))
        .await
        .expect("spawn kble-dump record");

    for frame in frames {
        plug.send(Bytes::copy_from_slice(frame))
            .await
            .expect("record send");
        let echo = plug.recv().await.expect("record echoes the frame");
        assert_eq!(echo.as_ref(), *frame, "record must echo the input frame");
    }

    plug.shutdown()
        .await
        .expect("record exits cleanly when its input closes");
    find_dump_file(&dir)
}

/// Record a few frames, then replay the dump file: replay must emit the same
/// frames in order and then **exit** once the file is exhausted — even though
/// its WS input is still open. (Regression: replay drained stdin on a spawned
/// task using `tokio::io::stdin()`'s uncancellable blocking read, which blocked
/// runtime shutdown, so replay hung after playback instead of terminating.)
#[tokio::test]
async fn replays_recorded_frames_and_exits() {
    let frames: [&[u8]; 3] = [b"first frame", b"the second one", b"third!"];
    let dump = record_dump(&frames).await;

    let mut plug = Plug::spawn(kble_dump(&["replay", dump.to_str().unwrap()]))
        .await
        .expect("spawn kble-dump replay");

    for expected in &frames {
        let got = plug.recv().await.expect("replay emits a frame");
        assert_eq!(got.as_ref(), *expected, "replay must reproduce the frame");
    }

    // The file is exhausted, so replay should terminate and close its output.
    let err = plug
        .recv_timeout(Duration::from_secs(5))
        .await
        .expect_err("replay output should end once the dump file is exhausted");
    let msg = err.to_string();
    assert!(
        msg.contains("reset") || msg.contains("closed"),
        "replay should exit after the file ends (reset/eof), got: {err}"
    );

    // Reap the now-exited process and confirm a clean exit status (mirroring the
    // kble-tcp lifecycle test); the WS sink is already dead, so closing it is a
    // no-op.
    plug.shutdown()
        .await
        .expect("replay should have exited cleanly once the dump file ended");
}
