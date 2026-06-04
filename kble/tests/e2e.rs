//! End-to-end tests for the `kble` orchestrator binary itself.
//!
//! The plug E2E tests (kble-eb90, kble-c2a) spawn a single plug and talk to it
//! directly. These instead launch the real `kble` process against a generated
//! spaghetti config and verify it forwards bytes across a link and shuts down
//! cleanly.
//!
//! The orchestrator launches `exec:` plugs internally, so a test cannot observe
//! their traffic from the outside. A `ws://` plug is the seam: the orchestrator
//! connects to it as a client, so the tests stand up in-process `ws://` plugs
//! (`kble_test_support::WsPlug`) as a link's endpoints and push bytes through
//! the real orchestrator process.
//!
//! The pipeline test goes further: it wires `ws://` endpoints around *real*
//! `exec:` plugs (`kble-eb90 encode`/`decode`) so a payload round-trips through
//! two orchestrator-launched processes, exercising the `exec:` spawn path and
//! multi-link wiring end to end.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use bytes::Bytes;
use kble_test_support::{WsPlug, WsPlugConn};
use proptest::prelude::*;
use tokio::process::{Child, Command};
use tokio::runtime::Runtime;

/// Build a `Command` running the freshly-built `kble` orchestrator against the
/// spaghetti config at `path`. Cargo exposes the binary path to this crate's
/// own integration tests via the env var, so no `escargot`/`assert_cmd` is
/// needed. `kill_on_drop` ensures a panicking test never leaks a live process.
fn kble(path: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_kble"));
    cmd.arg("--spaghetti")
        .arg(path)
        // Bound the orchestrator's own shutdown (default grace is 10s) well
        // below the test's wait deadline, so a slow plug exit can never race
        // the deadline into a confusing timeout panic.
        .arg("--termination-grace-period-secs")
        .arg("2")
        .kill_on_drop(true);
    cmd
}

/// Write `yaml` to a uniquely-named file under the per-test-binary temp dir
/// Cargo provides, and return its path. Cargo owns that dir, so no cleanup is
/// needed; the unique counter keeps parallel tests from clobbering each other.
fn write_spaghetti(yaml: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("spaghetti-{n}.yaml"));
    std::fs::write(&path, yaml).expect("write spaghetti config");
    path
}

/// A spaghetti config with two `ws://` plugs and a single `source -> sink` link.
fn forward_config(source: &WsPlug, sink: &WsPlug) -> PathBuf {
    let yaml = format!(
        "plugs:\n  source: {}\n  sink: {}\nlinks:\n  source: sink\n",
        source.url(),
        sink.url(),
    );
    write_spaghetti(&yaml)
}

/// Spawn the orchestrator for a single `source -> sink` link and hand back the
/// two connected endpoints.
///
/// Both plugs are bound before the spawn (their URLs go into the config) and
/// accepted concurrently with `tokio::join!`: the orchestrator dials its plugs
/// one at a time and blocks on each handshake, so accepting them sequentially
/// could deadlock if its (HashMap-ordered) connection order differs.
async fn spawn_forwarder() -> (Child, WsPlugConn, WsPlugConn) {
    let source = WsPlug::bind().await.expect("bind source plug");
    let sink = WsPlug::bind().await.expect("bind sink plug");
    let config = forward_config(&source, &sink);

    let child = kble(&config).spawn().expect("spawn kble orchestrator");

    let (source_conn, sink_conn) = tokio::join!(source.accept(), sink.accept());
    let source_conn = source_conn.expect("orchestrator connects to source plug");
    let sink_conn = sink_conn.expect("orchestrator connects to sink plug");
    (child, source_conn, sink_conn)
}

/// Absolute path to a sibling plug binary (e.g. `kble-eb90`) in the same Cargo
/// target dir as the orchestrator. Cargo only sets `CARGO_BIN_EXE_*` for this
/// crate's own binary, so a cross-crate plug is located relative to it. The
/// binary must already be built: `cargo test` builds every workspace member, so
/// CI is fine; for an isolated `cargo test -p kble`, `cargo build -p <name>` first.
fn plug_bin(name: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_BIN_EXE_kble")).with_file_name(name);
    assert!(
        path.exists(),
        "{name} binary not found at {path:?}; build the workspace first \
         (`cargo test` builds all members; or `cargo build -p {name}`)"
    );
    path
}

/// Spawn the orchestrator wired as a pipeline through two real `exec:` plugs and
/// hand back the observable `ws://` ends:
///
/// ```text
/// gen (ws) -> enc (exec: kble-eb90 encode) -> dec (exec: kble-eb90 decode) -> sink (ws)
/// ```
///
/// Bytes sent on `gen` are EB90-framed by `enc`, de-framed by `dec`, and must
/// arrive on `sink` unchanged.
async fn spawn_pipeline() -> (Child, WsPlugConn, WsPlugConn) {
    let gen = WsPlug::bind().await.expect("bind gen plug");
    let sink = WsPlug::bind().await.expect("bind sink plug");
    let gen_url = gen.url();
    let sink_url = sink.url();
    let eb90 = plug_bin("kble-eb90");
    let eb90 = eb90.display();
    // The path is interpolated unquoted on purpose: an absolute path puts the
    // arg-separator space into `url.path()` as `%20`, which is exactly the
    // percent-decoding the orchestrator (and this pipeline) must handle —
    // quoting would make the URL opaque and bypass it. This assumes the cargo
    // target path has no spaces or URL-reserved chars (`#`/`?`), which holds in
    // CI and typical checkouts; otherwise `sh -c` would word-split the path.
    let yaml = format!(
        "plugs:\n  gen: {gen_url}\n  enc: exec:{eb90} encode\n  dec: exec:{eb90} decode\n  \
         sink: {sink_url}\nlinks:\n  gen: enc\n  enc: dec\n  dec: sink\n"
    );
    let config = write_spaghetti(&yaml);

    let child = kble(&config).spawn().expect("spawn kble orchestrator");

    let (gen_conn, sink_conn) = tokio::join!(gen.accept(), sink.accept());
    let gen_conn = gen_conn.expect("orchestrator connects to gen plug");
    let sink_conn = sink_conn.expect("orchestrator connects to sink plug");
    (child, gen_conn, sink_conn)
}

/// Drive a clean orchestrator shutdown and assert it exits successfully.
///
/// Dropping the source closes its TCP transport, which the orchestrator reads
/// as the source link ending — its cue to quit. On the way out it closes the
/// sink and exits. Draining the sink concurrently keeps reading until `recv`
/// returns `Err` — on EOF once the orchestrator exits and the transport closes,
/// or on `recv`'s own deadline — so the `join` completes promptly instead of
/// parking on a never-read stream (which would also build up backpressure on
/// the orchestrator's send side). The outer `child.wait` deadline is what turns
/// a failure-to-shut-down into a test failure rather than a hang.
async fn shutdown_and_assert_clean_exit(
    mut child: Child,
    source: WsPlugConn,
    mut sink: WsPlugConn,
) {
    drop(source);
    let drain_sink = async { while sink.recv().await.is_ok() {} };
    let wait = async {
        tokio::time::timeout(Duration::from_secs(10), child.wait())
            .await
            .expect("orchestrator should exit after its source link closes")
            .expect("wait for orchestrator")
    };
    let (_, status) = tokio::join!(drain_sink, wait);
    assert!(status.success(), "orchestrator exited with {status}");
}

/// A small current-thread runtime to drive the async body of `proptest!` cases.
fn runtime() -> Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime")
}

/// The orchestrator forwards a binary frame from a link's source to its sink
/// unchanged.
#[tokio::test]
async fn forwards_a_frame_from_source_to_sink() {
    let (child, mut source, mut sink) = spawn_forwarder().await;

    let payload = Bytes::from_static(b"hello, orchestrator");
    source.send(payload.clone()).await.expect("source send");
    let got = sink.recv().await.expect("sink recv");
    assert_eq!(got, payload);

    shutdown_and_assert_clean_exit(child, source, sink).await;
}

/// Several frames sent back-to-back arrive at the sink unchanged and in the
/// order they were sent: the orchestrator must not reorder or coalesce them.
#[tokio::test]
async fn forwards_frames_in_order() {
    let (child, mut source, mut sink) = spawn_forwarder().await;

    let payloads: Vec<Bytes> = (0..16u8)
        .map(|i| Bytes::from(vec![i; usize::from(i) + 1]))
        .collect();
    for payload in &payloads {
        source.send(payload.clone()).await.expect("source send");
    }
    for expected in &payloads {
        let got = sink.recv().await.expect("sink recv");
        assert_eq!(&got, expected);
    }

    shutdown_and_assert_clean_exit(child, source, sink).await;
}

/// With a single link, the orchestrator must exit cleanly once that link's
/// source closes — even when no bytes ever crossed it. This is the regression
/// test for the final-link shutdown: signalling quit to the (now zero) other
/// links must not be treated as an error.
#[tokio::test]
async fn exits_cleanly_when_the_only_link_closes() {
    let (child, source, sink) = spawn_forwarder().await;
    shutdown_and_assert_clean_exit(child, source, sink).await;
}

/// A payload round-trips through a pipeline of two real `exec:` plugs the
/// orchestrator launches: `gen -> kble-eb90 encode -> kble-eb90 decode -> sink`.
/// `encode` frames it, `decode` removes the framing, so it must arrive on `sink`
/// byte-for-byte — proving the orchestrator spawns `exec:` plugs and wires a
/// multi-link chain correctly.
#[tokio::test]
async fn roundtrips_through_an_exec_plug_pipeline() {
    let (child, mut gen, mut sink) = spawn_pipeline().await;

    let payload = Bytes::from_static(b"multi-hop payload through the eb90 codec");
    gen.send(payload.clone()).await.expect("gen send");
    let got = sink.recv().await.expect("sink recv");
    assert_eq!(got, payload);

    shutdown_and_assert_clean_exit(child, gen, sink).await;
}

proptest! {
    // Each case spawns an orchestrator process plus two in-process ws servers,
    // so keep the count modest. Integration tests have no crate-root source
    // dir, so disable the on-disk regression file.
    #![proptest_config(ProptestConfig {
        cases: 24,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    /// An arbitrary sequence of arbitrary-byte frames is forwarded across the
    /// link unchanged and in order.
    #[test]
    fn forwards_arbitrary_frame_sequences_unchanged(
        frames in proptest::collection::vec(
            proptest::collection::vec(any::<u8>(), 1..256),
            1..16,
        )
    ) {
        let received: Vec<Vec<u8>> = runtime().block_on(async {
            let (mut child, mut source, mut sink) = spawn_forwarder().await;
            for frame in &frames {
                source.send(Bytes::copy_from_slice(frame)).await.expect("source send");
            }
            let mut received = Vec::with_capacity(frames.len());
            for _ in &frames {
                received.push(sink.recv().await.expect("sink recv").to_vec());
            }
            // This case only asserts on the forwarded bytes, not the exit
            // status. `Child::kill().await` is `start_kill()` + `wait().await`
            // internally, so it both SIGKILLs *and reaps* the child right here
            // (not relying on drop) — no zombie when this case's current-thread
            // runtime is torn down immediately after.
            child.kill().await.ok();
            received
        });
        prop_assert_eq!(received, frames);
    }
}
