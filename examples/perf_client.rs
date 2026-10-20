//! Headless perf client: attaches to a running daemon like a real client and
//! measures the snapshot pipeline end to end (frames/sec, wire bytes, decode
//! cost, revision drops, input-echo latency) without a host terminal.
//!
//! Usually launched via `scripts/perf/run`, which starts a dedicated daemon.
//! Direct use:
//!
//! ```sh
//! cargo run --release --example perf_client -- --socket /path/fut.sock flood
//! cargo run --release --example perf_client -- --socket /path/fut.sock styled
//! cargo run --release --example perf_client -- --socket /path/fut.sock latency
//! ```
//!
//! The daemon's pane must be running a plain `/bin/sh` reading stdin (the
//! `flood`/`styled` scenarios type shell commands into it).

use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use fut::{
    domain::{
        MouseEvent, MouseEventKind, MouseWheelDirection, ScreenDelta, ScreenSnapshot, TerminalId,
        TerminalSize,
    },
    protocol::{
        ClientMessage, ClientMode, Envelope, ServerMessage,
        chunked::{Assembly, ScreenAssembler},
        codec, decode_payload, encode_payload,
        local::LocalHello,
    },
};
use futures_util::{SinkExt, StreamExt};
use tokio::{net::UnixStream, time};
use tokio_util::codec::Framed;
use uuid::Uuid;

const DEADLINE: Duration = Duration::from_secs(180);

/// A daemon connection that reassembles chunked screens, so a screen sent as
/// a row-range chunk sequence counts as one frame.
struct Connection {
    framed: Framed<UnixStream, tokio_util::codec::LengthDelimitedCodec>,
    screens: ScreenAssembler,
    /// The sequence in progress so far, kept across a cancelled read.
    pending: Frame,
}

impl Connection {
    /// The next message, with a chunk sequence's wire bytes and decode time
    /// (including reassembly) summed. Cancel-safe.
    async fn next_frame(&mut self) -> Option<anyhow::Result<Frame>> {
        loop {
            let frame = match self.framed.next().await? {
                Ok(frame) => frame,
                Err(error) => return Some(Err(error.into())),
            };
            let started = Instant::now();
            let envelope: Envelope<ServerMessage> = match decode_payload(&frame) {
                Ok(envelope) => envelope,
                Err(error) => return Some(Err(error.into())),
            };
            let chunk = matches!(
                envelope.message,
                ServerMessage::ScreenBegin { .. }
                    | ServerMessage::ScreenRows { .. }
                    | ServerMessage::ScreenEnd { .. }
            );
            let assembled = self.screens.accept(envelope.message);
            self.pending.decode += started.elapsed();
            self.pending.wire_bytes += frame.len();
            self.pending.largest_frame = self.pending.largest_frame.max(frame.len());
            self.pending.frames += usize::from(chunk);
            match assembled {
                Assembly::Ready(message) => {
                    let mut ready = std::mem::take(&mut self.pending);
                    ready.envelope = Envelope {
                        request_id: envelope.request_id,
                        message,
                    };
                    return Some(Ok(ready));
                }
                Assembly::Pending => {}
                Assembly::Discarded(discarded) => {
                    return Some(Err(anyhow::anyhow!(
                        "discarded chunked screens: {discarded:?}"
                    )));
                }
            }
        }
    }
}

struct Args {
    socket: String,
    scenario: String,
    columns: u16,
    rows: u16,
    count: Option<u64>,
}

fn parse_args() -> anyhow::Result<Args> {
    let mut args = Args {
        socket: String::new(),
        scenario: String::new(),
        columns: 200,
        rows: 50,
        count: None,
    };
    let mut raw = std::env::args().skip(1);
    while let Some(arg) = raw.next() {
        let mut value = |name: &str| raw.next().context(format!("{name} requires a value"));
        match arg.as_str() {
            "--socket" => args.socket = value("--socket")?,
            "--columns" => args.columns = value("--columns")?.parse()?,
            "--rows" => args.rows = value("--rows")?.parse()?,
            "--count" => args.count = Some(value("--count")?.parse()?),
            scenario if !scenario.starts_with('-') => args.scenario = scenario.into(),
            other => bail!("unknown argument {other}"),
        }
    }
    if args.socket.is_empty() || args.scenario.is_empty() {
        bail!(
            "usage: perf_client --socket PATH [--columns N] [--rows N] [--count N] flood|styled|dense|partial|scroll|wheel|latency"
        );
    }
    Ok(args)
}

async fn send(
    connection: &mut Connection,
    request_id: Option<Uuid>,
    message: ClientMessage,
) -> anyhow::Result<()> {
    let payload = encode_payload(&Envelope {
        request_id,
        message,
    })?;
    connection.framed.send(payload.into()).await?;
    Ok(())
}

/// One decoded message plus its wire size and decode cost. A chunked screen
/// is one message made of `frames` chunk frames.
struct Frame {
    envelope: Envelope<ServerMessage>,
    wire_bytes: usize,
    largest_frame: usize,
    frames: usize,
    decode: Duration,
}

impl Default for Frame {
    fn default() -> Self {
        Self {
            envelope: Envelope {
                request_id: None,
                message: ServerMessage::Detached,
            },
            wire_bytes: 0,
            largest_frame: 0,
            frames: 0,
            decode: Duration::ZERO,
        }
    }
}

async fn receive(connection: &mut Connection) -> anyhow::Result<Frame> {
    time::timeout(DEADLINE, connection.next_frame())
        .await
        .context("timed out waiting for a daemon frame")?
        .context("daemon closed the connection")?
}

fn screen_text(screen: &ScreenSnapshot) -> String {
    let columns = screen.size.columns as usize;
    let mut text = String::with_capacity(screen.cells.len() + screen.size.rows as usize);
    for row in screen.cells.chunks(columns) {
        for cell in row {
            text.push_str(&cell.contents);
        }
        text.push('\n');
    }
    text
}

/// Standalone stand-in for the client's `PaneState`: materializes a grid from
/// full snapshots and applies domain-validated deltas.
#[derive(Default)]
struct MaterializedScreen {
    screen: Option<ScreenSnapshot>,
}

impl MaterializedScreen {
    fn apply_full(&mut self, screen: ScreenSnapshot) -> &ScreenSnapshot {
        self.screen.insert(screen)
    }

    /// Applies `delta`, or returns `None` if it doesn't apply — a real client would
    /// send `RefreshTerminal` here; this harness just discards the frame.
    fn apply_delta(&mut self, delta: ScreenDelta) -> Option<&ScreenSnapshot> {
        let current = self.screen.as_mut()?;
        if current.apply_delta(delta).is_err() {
            return None;
        }
        self.screen.as_ref()
    }

    /// Applies `message` if it's a `Snapshot`/`SnapshotDelta` for
    /// `terminal_id`, returning the resulting grid. Must be called for every
    /// frame received for that terminal — including ones a caller doesn't
    /// otherwise care about — since a dropped delta desyncs the base every
    /// later delta is diffed against.
    fn apply(
        &mut self,
        terminal_id: TerminalId,
        message: ServerMessage,
    ) -> Option<&ScreenSnapshot> {
        match message {
            ServerMessage::Snapshot {
                terminal_id: id,
                screen,
            } if id == terminal_id => Some(self.apply_full(screen)),
            ServerMessage::SnapshotDelta {
                terminal_id: id,
                delta,
            } if id == terminal_id => self.apply_delta(delta),
            _ => None,
        }
    }
}

#[derive(Default)]
struct FloodStats {
    snapshots: u64,
    chunked: u64,
    chunk_frames: u64,
    full_snapshots: u64,
    deltas: u64,
    wire_bytes: u64,
    full_wire_bytes: u64,
    delta_wire_bytes: u64,
    max_frame: usize,
    decode_total: Duration,
    full_decode: Duration,
    delta_decode: Duration,
    first_revision: Option<u64>,
    last_revision: u64,
}

impl FloodStats {
    fn accept(&mut self, revision: u64, frame: &Frame, is_delta: bool) {
        let (wire_bytes, decode) = (frame.wire_bytes, frame.decode);
        self.snapshots += 1;
        if frame.frames > 0 {
            self.chunked += 1;
            self.chunk_frames += frame.frames as u64;
        }
        self.wire_bytes += wire_bytes as u64;
        if is_delta {
            self.deltas += 1;
            self.delta_wire_bytes += wire_bytes as u64;
            self.delta_decode += decode;
        } else {
            self.full_snapshots += 1;
            self.full_wire_bytes += wire_bytes as u64;
            self.full_decode += decode;
        }
        self.max_frame = self.max_frame.max(frame.largest_frame);
        self.decode_total += decode;
        self.first_revision.get_or_insert(revision);
        self.last_revision = revision;
    }

    fn report(&self, label: &str, elapsed: Duration) {
        let seconds = elapsed.as_secs_f64();
        let revisions = self
            .first_revision
            .map(|first| self.last_revision - first + 1)
            .unwrap_or_default();
        println!("== {label} ==");
        println!("wall time            {seconds:>10.2} s");
        println!("snapshots received   {:>10}", self.snapshots);
        println!(
            "snapshots/sec        {:>10.1}",
            self.snapshots as f64 / seconds
        );
        println!(
            "revisions spanned    {revisions:>10}  (coalesced away: {})",
            revisions.saturating_sub(self.snapshots)
        );
        println!(
            "deltas / full        {:>10} / {}  ({:.1}% deltas)",
            self.deltas,
            self.full_snapshots,
            100.0 * self.deltas as f64 / self.snapshots.max(1) as f64
        );
        println!(
            "wire received        {:>10.1} MiB  ({:.1} MiB/s)",
            self.wire_bytes as f64 / (1024.0 * 1024.0),
            self.wire_bytes as f64 / (1024.0 * 1024.0) / seconds
        );
        println!(
            "  delta wire         {:>10.1} MiB",
            self.delta_wire_bytes as f64 / (1024.0 * 1024.0)
        );
        println!(
            "  full wire          {:>10.1} MiB",
            self.full_wire_bytes as f64 / (1024.0 * 1024.0)
        );
        println!("largest frame        {:>10} bytes", self.max_frame);
        println!(
            "chunked screens      {:>10}  ({} frames)",
            self.chunked, self.chunk_frames
        );
        println!(
            "avg decode           {:>10.0} us",
            self.decode_total.as_micros() as f64 / self.snapshots.max(1) as f64
        );
        println!(
            "  avg delta / full   {:>10.0} / {:.0} us",
            self.delta_decode.as_micros() as f64 / self.deltas.max(1) as f64,
            self.full_decode.as_micros() as f64 / self.full_snapshots.max(1) as f64
        );
        println!(
            "  avg delta / full   {:>10.0} / {:.0} bytes",
            self.delta_wire_bytes as f64 / self.deltas.max(1) as f64,
            self.full_wire_bytes as f64 / self.full_snapshots.max(1) as f64
        );
        println!(
            "decode total         {:>10.1} ms",
            self.decode_total.as_secs_f64() * 1000.0
        );
    }
}

/// Type a command into the pane's shell and measure until its sentinel shows
/// up in a snapshot. `materialized` must already reflect this terminal's
/// current grid (from [`settle`]) and keeps being fed every frame seen for
/// it, including ones this scenario's `terminal_id` filter would otherwise
/// skip — a delta applies only against the exact base revision the daemon
/// sent it against, so missing even one desyncs every later delta.
async fn flood(
    connection: &mut Connection,
    terminal_id: TerminalId,
    materialized: &mut MaterializedScreen,
    label: &str,
    command: &str,
) -> anyhow::Result<()> {
    let command = format!("{command}; echo PERF_\\DONE\n");
    let started = Instant::now();
    send(
        connection,
        None,
        ClientMessage::Input {
            bytes: command.into_bytes(),
        },
    )
    .await?;
    let mut stats = FloodStats::default();
    loop {
        let mut frame = receive(connection).await?;
        let is_delta = matches!(frame.envelope.message, ServerMessage::SnapshotDelta { .. });
        let message = std::mem::replace(&mut frame.envelope.message, ServerMessage::Detached);
        let Some(screen) = materialized.apply(terminal_id, message) else {
            continue;
        };
        stats.accept(screen.revision, &frame, is_delta);
        if screen_text(screen).contains("PERF_DONE") {
            break;
        }
    }
    stats.report(label, started.elapsed());
    verify_against_refresh(connection, terminal_id, materialized).await
}

/// Let output settle, then ask for a fresh full screen and check that the
/// grid materialized from every delta so far matches it exactly.
async fn verify_against_refresh(
    connection: &mut Connection,
    terminal_id: TerminalId,
    materialized: &mut MaterializedScreen,
) -> anyhow::Result<()> {
    while let Ok(Some(Ok(frame))) =
        time::timeout(Duration::from_millis(300), connection.next_frame()).await
    {
        materialized.apply(terminal_id, frame.envelope.message);
    }
    let Some(applied) = materialized.screen.clone() else {
        bail!("no materialized screen to verify");
    };
    send(
        connection,
        None,
        ClientMessage::RefreshTerminal { terminal_id },
    )
    .await?;
    loop {
        let frame = receive(connection).await?;
        if let ServerMessage::Snapshot {
            terminal_id: id,
            screen,
        } = frame.envelope.message
            && id == terminal_id
        {
            let matches = screen.cells == applied.cells
                && screen.cursor == applied.cursor
                && screen.hyperlinks == applied.hyperlinks;
            println!(
                "matches full refresh {:>10}",
                if matches { "yes" } else { "NO" }
            );
            if !matches {
                bail!("materialized grid diverged from a fresh full screen");
            }
            return Ok(());
        }
    }
}

/// Scrollback viewport scrolling: fill the scrollback, then wheel up and
/// back down through it at a human-ish cadence. `count` is the number of
/// wheel events in each direction (default 60).
async fn wheel(
    connection: &mut Connection,
    terminal_id: TerminalId,
    materialized: &mut MaterializedScreen,
    count: Option<u64>,
) -> anyhow::Result<()> {
    let fill = "seq 1 5000; echo PERF_\\DONE\n";
    send(
        connection,
        None,
        ClientMessage::Input {
            bytes: fill.as_bytes().to_vec(),
        },
    )
    .await?;
    loop {
        let frame = receive(connection).await?;
        if let Some(screen) = materialized.apply(terminal_id, frame.envelope.message)
            && screen_text(screen).contains("PERF_DONE")
        {
            break;
        }
    }
    while let Ok(Some(Ok(frame))) =
        time::timeout(Duration::from_millis(300), connection.next_frame()).await
    {
        materialized.apply(terminal_id, frame.envelope.message);
    }
    let events = count.unwrap_or(60);
    for (label, direction) in [
        ("wheel (scrollback up)", MouseWheelDirection::Up),
        ("wheel (scrollback down)", MouseWheelDirection::Down),
    ] {
        let started = Instant::now();
        let mut stats = FloodStats::default();
        let mut next_event = Instant::now();
        let mut sent = 0;
        loop {
            if sent < events && Instant::now() >= next_event {
                send(
                    connection,
                    None,
                    ClientMessage::MouseInput {
                        terminal_id,
                        event: MouseEvent {
                            kind: MouseEventKind::Wheel { direction },
                            column: 1,
                            row: 1,
                            modifiers: Default::default(),
                            buttons: Default::default(),
                        },
                    },
                )
                .await?;
                sent += 1;
                next_event += Duration::from_millis(20);
            }
            let wait = if sent < events {
                next_event.saturating_duration_since(Instant::now())
            } else {
                Duration::from_millis(300)
            };
            let Ok(frame) = time::timeout(wait, connection.next_frame()).await else {
                if sent < events {
                    continue;
                }
                break;
            };
            let mut frame = frame.context("daemon closed the connection")??;
            let is_delta = matches!(frame.envelope.message, ServerMessage::SnapshotDelta { .. });
            let message = std::mem::replace(&mut frame.envelope.message, ServerMessage::Detached);
            if let Some(screen) = materialized.apply(terminal_id, message) {
                stats.accept(screen.revision, &frame, is_delta);
            }
        }
        stats.report(label, started.elapsed());
    }
    verify_against_refresh(connection, terminal_id, materialized).await
}

/// Keystroke echo latency: send one character, wait for the snapshot in which
/// it appears. Runs twice — spaced like human typing, and back-to-back like
/// key repeat, which is bounded by the daemon's snapshot pacing.
async fn latency(
    connection: &mut Connection,
    terminal_id: TerminalId,
    materialized: &mut MaterializedScreen,
) -> anyhow::Result<()> {
    let mut expected = 0usize;
    for (label, gap) in [
        ("latency (typing, 30ms gaps)", Duration::from_millis(30)),
        ("latency (burst, back-to-back)", Duration::ZERO),
    ] {
        latency_round(
            connection,
            terminal_id,
            label,
            gap,
            &mut expected,
            materialized,
        )
        .await?;
    }
    Ok(())
}

async fn latency_round(
    connection: &mut Connection,
    terminal_id: TerminalId,
    label: &str,
    gap: Duration,
    expected: &mut usize,
    materialized: &mut MaterializedScreen,
) -> anyhow::Result<()> {
    let iterations = 40;
    let mut samples = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        time::sleep(gap).await;
        *expected += 1;
        let expected = *expected;
        let started = Instant::now();
        send(
            connection,
            None,
            ClientMessage::Input {
                bytes: b"x".to_vec(),
            },
        )
        .await?;
        loop {
            let frame = receive(connection).await?;
            let Some(screen) = materialized.apply(terminal_id, frame.envelope.message) else {
                continue;
            };
            let count = screen
                .cells
                .iter()
                .filter(|cell| cell.contents == "x")
                .count();
            if count >= expected {
                samples.push(started.elapsed());
                break;
            }
        }
    }
    samples.sort();
    let at = |fraction: f64| samples[(samples.len() as f64 * fraction) as usize - 1];
    println!("== {label} ==");
    println!("iterations           {:>10}", samples.len());
    println!(
        "echo p50             {:>10.1} ms",
        at(0.50).as_secs_f64() * 1000.0
    );
    println!(
        "echo p95             {:>10.1} ms",
        at(0.95).as_secs_f64() * 1000.0
    );
    println!(
        "echo max             {:>10.1} ms",
        samples.last().unwrap().as_secs_f64() * 1000.0
    );
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = parse_args()?;
    let stream = UnixStream::connect(&args.socket)
        .await
        .with_context(|| format!("connecting to {}", args.socket))?;
    let mut connection = Connection {
        framed: Framed::new(stream, codec()),
        screens: ScreenAssembler::default(),
        pending: Frame::default(),
    };
    let hello = LocalHello::new(
        ClientMode::Interactive {
            size: TerminalSize {
                columns: args.columns,
                rows: args.rows,
            },
            selector: None,
        },
        "fut-perf",
    );
    send(
        &mut connection,
        Some(Uuid::new_v4()),
        ClientMessage::LocalHello(hello.clone()),
    )
    .await?;
    let welcome = receive(&mut connection).await?;
    let terminal_id = match welcome.envelope.message {
        ServerMessage::LocalWelcome(welcome) => {
            hello.accept(&welcome)?;
            welcome
                .selected
                .context("daemon did not select a terminal")?
                .focused
                .terminal_id
        }
        other => bail!("expected a welcome with a focused terminal, received {other:?}"),
    };

    // A real client resizes its focused pane to match its host terminal
    // right after attaching; do the same so the pane actually matches
    // --columns/--rows instead of staying at whatever size the daemon spawned
    // it with.
    send(
        &mut connection,
        Some(Uuid::new_v4()),
        ClientMessage::Resize {
            terminal_id,
            size: TerminalSize {
                columns: args.columns,
                rows: args.rows,
            },
        },
    )
    .await?;

    // Let the initial attach and resize snapshots settle before measuring,
    // but keep applying them to `materialized` — the first thing a scenario
    // sees afterward may well be a delta, which needs an already-current
    // base grid to apply against.
    let mut materialized = MaterializedScreen::default();
    time::sleep(Duration::from_millis(300)).await;
    while let Ok(Some(Ok(frame))) =
        time::timeout(Duration::from_millis(100), connection.next_frame()).await
    {
        materialized.apply(terminal_id, frame.envelope.message);
    }

    match args.scenario.as_str() {
        "flood" => {
            let command = format!("seq 1 {}", args.count.unwrap_or(100_000));
            flood(
                &mut connection,
                terminal_id,
                &mut materialized,
                "flood (plain)",
                &command,
            )
            .await?;
        }
        "styled" => {
            let command = format!(
                "awk 'BEGIN{{for(i=0;i<{};i++)printf \"\\033[3%dm%8d styled\\033[1m words\\033[0m here\\n\", i%8, i}}'",
                args.count.unwrap_or(100_000)
            );
            flood(
                &mut connection,
                terminal_id,
                &mut materialized,
                "flood (styled)",
                &command,
            )
            .await?;
        }
        "dense" => {
            // vtebench dense_cells / animated-orb workload: full-screen
            // repaints with a unique truecolor fg+bg per cell, no scrolling.
            // `--count` is the number of frames here (default 240).
            let frames = args.count.unwrap_or(240).min(10_000);
            let command = format!(
                "awk -v cols={} -v rows={} -v frames={frames} 'BEGIN{{for(f=0;f<frames;f++){{for(r=0;r<rows;r++){{s=sprintf(\"\\033[%d;1H\",r+1);for(c=0;c<cols;c++){{s=s sprintf(\"\\033[38;2;%d;%d;%dm\\033[48;2;%d;%d;%dmo\",(r*5+f*7)%256,(c*3+f*11)%256,(r+c+f*13)%256,(255-r*5)%256,(255-c*3)%256,(255-f*13)%256)}}printf \"%s\",s}}}}printf \"\\033[0m\\033[2J\\033[H\"}}'",
                args.columns, args.rows
            );
            flood(
                &mut connection,
                terminal_id,
                &mut materialized,
                "dense (orb-like)",
                &command,
            )
            .await?;
        }
        "partial" => {
            // A small animated region on an otherwise static screen — the
            // realistic amp-orb shape (the orb is a badge, not the whole
            // pane). `--count` is the number of frames (default 600).
            let frames = args.count.unwrap_or(600).min(20_000);
            let command = format!(
                "awk -v frames={frames} 'BEGIN{{for(f=0;f<frames;f++){{for(r=20;r<30;r++){{s=sprintf(\"\\033[%d;81H\",r+1);for(c=0;c<40;c++){{s=s sprintf(\"\\033[38;2;%d;%d;%dm\\033[48;2;%d;%d;%dmo\",(r*5+f*7)%256,(c*3+f*11)%256,(r+c+f*13)%256,(255-r*5)%256,(255-c*3)%256,(255-f*13)%256)}}printf \"%s\",s}}}}printf \"\\033[0m\\033[2J\\033[H\"}}'"
            );
            flood(
                &mut connection,
                terminal_id,
                &mut materialized,
                "partial (small orb)",
                &command,
            )
            .await?;
        }
        "scroll" => {
            // Paced output, like a build log or `tail -f`: four new lines
            // per burst, so most of each screen is the previous one shifted
            // up. `--count` is the number of bursts (default 400).
            let bursts = args.count.unwrap_or(400).min(100_000);
            let command = format!(
                "i=0; while [ $i -lt {bursts} ]; do printf 'step %06d compiling crate fut-%d (lib) in the workspace\\n' $i $i $i $i $i $i $i $i; sleep 0.01; i=$((i+1)); done"
            );
            flood(
                &mut connection,
                terminal_id,
                &mut materialized,
                "scroll (paced output)",
                &command,
            )
            .await?;
        }
        "wheel" => wheel(&mut connection, terminal_id, &mut materialized, args.count).await?,
        "latency" => latency(&mut connection, terminal_id, &mut materialized).await?,
        other => bail!("unknown scenario {other}"),
    }
    Ok(())
}
