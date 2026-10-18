//! Screens too large for one frame travel as row-range chunks.
//!
//! A screen message (full screen, copy-mode screen, or delta) whose rows hold
//! more than [`crate::domain::MAX_VISIBLE_CELLS`] cells is sent as a
//! [`ServerMessage::ScreenBegin`] carrying everything but its rows, one or
//! more [`ServerMessage::ScreenRows`] carrying contiguous runs of rows, and a
//! [`ServerMessage::ScreenEnd`]. All of them name the screen's revision.
//!
//! The daemon's writer splits a screen only after taking it off the
//! connection's queue and writes the whole sequence before taking anything
//! else, so the queue's coalescing (a newer screen replacing an unsent one)
//! only ever replaces whole screens, and sequences never interleave on the
//! wire. [`ScreenAssembler`] turns a sequence back into the one message it
//! stands for, so a receiver presents only complete screens; a partial screen
//! is discarded when a newer one for the same terminal begins, and any other
//! malformed sequence is discarded and reported so the receiver can ask for a
//! fresh screen.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::ServerMessage;
use crate::domain::{
    DeltaRow, ScreenDelta, ScreenSnapshot, TerminalId, TerminalSize, TerminalSizeError,
};

/// Everything about a chunked screen except its rows, and how many rows follow.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ScreenBegin {
    /// Rows the following [`ServerMessage::ScreenRows`] frames carry in total.
    #[serde(rename = "n")]
    pub rows: u16,
    #[serde(rename = "f")]
    pub screen: ChunkedScreen,
}

/// The message a chunk sequence stands for, with its rows taken out: a full
/// screen's `cells` and a delta's `rows` are empty. Full screens are followed
/// by every row in order; deltas by their replacement rows in ascending order.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChunkedScreen {
    Snapshot(ScreenSnapshot),
    CopyModeSnapshot(ScreenSnapshot),
    CopyModeFinalized(ScreenSnapshot),
    CopyModeCancelled(ScreenSnapshot),
    SnapshotDelta(ScreenDelta),
}

impl ChunkedScreen {
    fn revision(&self) -> u64 {
        match self {
            Self::Snapshot(screen)
            | Self::CopyModeSnapshot(screen)
            | Self::CopyModeFinalized(screen)
            | Self::CopyModeCancelled(screen) => screen.revision,
            Self::SnapshotDelta(delta) => delta.revision,
        }
    }

    fn size(&self) -> TerminalSize {
        match self {
            Self::Snapshot(screen)
            | Self::CopyModeSnapshot(screen)
            | Self::CopyModeFinalized(screen)
            | Self::CopyModeCancelled(screen) => screen.size,
            Self::SnapshotDelta(delta) => delta.size,
        }
    }

    fn into_message(self, terminal_id: TerminalId) -> ServerMessage {
        match self {
            Self::Snapshot(screen) => ServerMessage::Snapshot {
                terminal_id,
                screen,
            },
            Self::CopyModeSnapshot(screen) => ServerMessage::CopyModeSnapshot {
                terminal_id,
                screen,
            },
            Self::CopyModeFinalized(screen) => ServerMessage::CopyModeFinalized {
                terminal_id,
                screen,
            },
            Self::CopyModeCancelled(screen) => ServerMessage::CopyModeCancelled {
                terminal_id,
                screen,
            },
            Self::SnapshotDelta(delta) => ServerMessage::SnapshotDelta { terminal_id, delta },
        }
    }
}

/// Split `message` into a chunk sequence if it is a screen whose rows hold
/// more than `max_cells` cells (the daemon uses [`crate::domain::MAX_VISIBLE_CELLS`], the
/// largest screen one frame is proven to hold); any other message comes back alone. Each
/// [`ServerMessage::ScreenRows`] holds at most `max_cells` cells, or one row
/// when a single row is wider. Rows are moved, not copied.
#[must_use]
pub fn split_screen(message: ServerMessage, max_cells: usize) -> Vec<ServerMessage> {
    let (terminal_id, screen, rows) = match message {
        ServerMessage::Snapshot {
            terminal_id,
            screen,
        } if screen.cells.len() > max_cells => {
            let (screen, rows) = take_rows(screen);
            (terminal_id, ChunkedScreen::Snapshot(screen), rows)
        }
        ServerMessage::CopyModeSnapshot {
            terminal_id,
            screen,
        } if screen.cells.len() > max_cells => {
            let (screen, rows) = take_rows(screen);
            (terminal_id, ChunkedScreen::CopyModeSnapshot(screen), rows)
        }
        ServerMessage::CopyModeFinalized {
            terminal_id,
            screen,
        } if screen.cells.len() > max_cells => {
            let (screen, rows) = take_rows(screen);
            (terminal_id, ChunkedScreen::CopyModeFinalized(screen), rows)
        }
        ServerMessage::CopyModeCancelled {
            terminal_id,
            screen,
        } if screen.cells.len() > max_cells => {
            let (screen, rows) = take_rows(screen);
            (terminal_id, ChunkedScreen::CopyModeCancelled(screen), rows)
        }
        ServerMessage::SnapshotDelta {
            terminal_id,
            mut delta,
        } if delta.rows.len() * usize::from(delta.size.columns) > max_cells => {
            let rows = std::mem::take(&mut delta.rows);
            (terminal_id, ChunkedScreen::SnapshotDelta(delta), rows)
        }
        other => return vec![other],
    };
    let revision = screen.revision();
    let columns = usize::from(screen.size().columns).max(1);
    // Spread rows evenly over as few chunks as the budget allows.
    let chunks = rows.len().div_ceil((max_cells / columns).max(1));
    let per_chunk = rows.len().div_ceil(chunks.max(1)).max(1);
    let mut frames = Vec::with_capacity(chunks + 2);
    frames.push(ServerMessage::ScreenBegin {
        terminal_id,
        begin: ScreenBegin {
            rows: u16::try_from(rows.len()).expect("a screen has at most u16::MAX rows"),
            screen,
        },
    });
    let mut rows = rows.into_iter();
    loop {
        let chunk: Vec<DeltaRow> = rows.by_ref().take(per_chunk).collect();
        if chunk.is_empty() {
            break;
        }
        frames.push(ServerMessage::ScreenRows {
            terminal_id,
            revision,
            rows: chunk,
        });
    }
    frames.push(ServerMessage::ScreenEnd {
        terminal_id,
        revision,
    });
    frames
}

fn take_rows(mut screen: ScreenSnapshot) -> (ScreenSnapshot, Vec<DeltaRow>) {
    let columns = usize::from(screen.size.columns);
    let mut cells = std::mem::take(&mut screen.cells).into_iter();
    let rows = (0..screen.size.rows)
        .map(|index| DeltaRow {
            index,
            cells: cells.by_ref().take(columns).collect(),
        })
        .collect();
    (screen, rows)
}

/// Why a chunk sequence was discarded.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ChunkError {
    #[error("screen chunk header has an invalid size: {0}")]
    InvalidSize(#[from] TerminalSizeError),
    #[error("screen chunk header carries rows or the wrong row count")]
    InvalidHeader,
    #[error("screen rows arrived without a matching screen in progress")]
    Unexpected,
    #[error("a screen for another terminal began before this one ended")]
    Interrupted,
    #[error("screen row {row} arrived out of order or outside the screen")]
    RowOrder { row: u16 },
    #[error("screen row {row} has {actual} cells; expected {expected}")]
    RowWidth {
        row: u16,
        expected: usize,
        actual: usize,
    },
    #[error("screen chunks carried more than the {expected} rows announced")]
    TooManyRows { expected: u16 },
    #[error("screen ended with {actual} of its {expected} rows")]
    Incomplete { expected: u16, actual: usize },
}

/// What a received message means once chunk sequences are accounted for.
/// Returned for every received message and consumed at once, so boxing the
/// message would only add an allocation per frame.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum Assembly {
    /// Handle this message as usual: an ordinary message, or the complete
    /// screen a chunk sequence stood for.
    Ready(ServerMessage),
    /// Part of a screen still arriving.
    Pending,
    /// These terminals' screens were discarded; ask for fresh ones.
    Discarded(Vec<(TerminalId, ChunkError)>),
}

/// Reassembles chunk sequences. At most one sequence is in progress, as the
/// daemon writes each one contiguously.
#[derive(Debug, Default)]
pub struct ScreenAssembler {
    partial: Option<Partial>,
    /// A discarded sequence whose remaining frames are ignored, so it is
    /// reported once.
    skipping: Option<(TerminalId, u64)>,
}

#[derive(Debug)]
struct Partial {
    terminal_id: TerminalId,
    revision: u64,
    rows: u16,
    received: usize,
    screen: ChunkedScreen,
}

impl Partial {
    fn new(terminal_id: TerminalId, begin: ScreenBegin) -> Result<Self, ChunkError> {
        let size = begin.screen.size();
        let cells = size.cell_count()?;
        match &begin.screen {
            ChunkedScreen::SnapshotDelta(delta) => {
                if !delta.rows.is_empty() || begin.rows > size.rows {
                    return Err(ChunkError::InvalidHeader);
                }
            }
            ChunkedScreen::Snapshot(screen)
            | ChunkedScreen::CopyModeSnapshot(screen)
            | ChunkedScreen::CopyModeFinalized(screen)
            | ChunkedScreen::CopyModeCancelled(screen) => {
                if !screen.cells.is_empty() || begin.rows != size.rows {
                    return Err(ChunkError::InvalidHeader);
                }
            }
        }
        let mut partial = Self {
            terminal_id,
            revision: begin.screen.revision(),
            rows: begin.rows,
            received: 0,
            screen: begin.screen,
        };
        match &mut partial.screen {
            ChunkedScreen::SnapshotDelta(delta) => {
                delta.rows.reserve_exact(usize::from(begin.rows))
            }
            ChunkedScreen::Snapshot(screen)
            | ChunkedScreen::CopyModeSnapshot(screen)
            | ChunkedScreen::CopyModeFinalized(screen)
            | ChunkedScreen::CopyModeCancelled(screen) => screen.cells.reserve_exact(cells),
        }
        Ok(partial)
    }

    fn push(&mut self, rows: Vec<DeltaRow>) -> Result<(), ChunkError> {
        let size = self.screen.size();
        let columns = usize::from(size.columns);
        for row in rows {
            if self.received >= usize::from(self.rows) {
                return Err(ChunkError::TooManyRows {
                    expected: self.rows,
                });
            }
            if row.cells.len() != columns {
                return Err(ChunkError::RowWidth {
                    row: row.index,
                    expected: columns,
                    actual: row.cells.len(),
                });
            }
            match &mut self.screen {
                ChunkedScreen::SnapshotDelta(delta) => {
                    let ascending = delta.rows.last().is_none_or(|last| row.index > last.index);
                    if !ascending || row.index >= size.rows {
                        return Err(ChunkError::RowOrder { row: row.index });
                    }
                    delta.rows.push(row);
                }
                ChunkedScreen::Snapshot(screen)
                | ChunkedScreen::CopyModeSnapshot(screen)
                | ChunkedScreen::CopyModeFinalized(screen)
                | ChunkedScreen::CopyModeCancelled(screen) => {
                    if usize::from(row.index) != self.received {
                        return Err(ChunkError::RowOrder { row: row.index });
                    }
                    screen.cells.extend(row.cells);
                }
            }
            self.received += 1;
        }
        Ok(())
    }

    fn finish(self) -> Result<ServerMessage, ChunkError> {
        if self.received != usize::from(self.rows) {
            return Err(ChunkError::Incomplete {
                expected: self.rows,
                actual: self.received,
            });
        }
        Ok(self.screen.into_message(self.terminal_id))
    }
}

impl ScreenAssembler {
    /// Account for one received message.
    pub fn accept(&mut self, message: ServerMessage) -> Assembly {
        match message {
            ServerMessage::ScreenBegin { terminal_id, begin } => {
                let revision = begin.screen.revision();
                let mut discarded = Vec::new();
                // A newer screen for the same terminal supersedes a partial
                // one; any other unfinished sequence is lost.
                if let Some(previous) = self.partial.take()
                    && previous.terminal_id != terminal_id
                {
                    discarded.push((previous.terminal_id, ChunkError::Interrupted));
                }
                self.skipping = None;
                match Partial::new(terminal_id, begin) {
                    Ok(partial) => self.partial = Some(partial),
                    Err(error) => {
                        self.skipping = Some((terminal_id, revision));
                        discarded.push((terminal_id, error));
                    }
                }
                if discarded.is_empty() {
                    Assembly::Pending
                } else {
                    Assembly::Discarded(discarded)
                }
            }
            ServerMessage::ScreenRows {
                terminal_id,
                revision,
                rows,
            } => {
                if self.skipping == Some((terminal_id, revision)) {
                    return Assembly::Pending;
                }
                let Some(partial) = self.partial.as_mut().filter(|partial| {
                    partial.terminal_id == terminal_id && partial.revision == revision
                }) else {
                    self.skipping = Some((terminal_id, revision));
                    return Assembly::Discarded(vec![(terminal_id, ChunkError::Unexpected)]);
                };
                match partial.push(rows) {
                    Ok(()) => Assembly::Pending,
                    Err(error) => {
                        self.partial = None;
                        self.skipping = Some((terminal_id, revision));
                        Assembly::Discarded(vec![(terminal_id, error)])
                    }
                }
            }
            ServerMessage::ScreenEnd {
                terminal_id,
                revision,
            } => {
                if self.skipping == Some((terminal_id, revision)) {
                    self.skipping = None;
                    return Assembly::Pending;
                }
                let matches = self.partial.as_ref().is_some_and(|partial| {
                    partial.terminal_id == terminal_id && partial.revision == revision
                });
                if !matches {
                    return Assembly::Discarded(vec![(terminal_id, ChunkError::Unexpected)]);
                }
                let partial = self.partial.take().expect("matched partial screen");
                match partial.finish() {
                    Ok(message) => Assembly::Ready(message),
                    Err(error) => Assembly::Discarded(vec![(terminal_id, error)]),
                }
            }
            other => Assembly::Ready(other),
        }
    }

    /// Whether a sequence is still arriving.
    #[must_use]
    pub fn in_progress(&self) -> bool {
        self.partial.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domain::{
            Cell, CellColor, CellStyle, Cursor, KittyGraphics, KittyImage, KittyPlacement,
            MAX_CELL_CONTENT_BYTES, MAX_KITTY_PLACEMENTS, MAX_SCREEN_HYPERLINK_BYTES,
            MAX_TERMINAL_COLUMNS, MAX_VISIBLE_CELLS, Rgb,
        },
        protocol::{Envelope, MAX_FRAME_LEN, decode_payload, encode_payload},
    };

    fn cursor() -> Cursor {
        Cursor {
            column: 0,
            row: 0,
            visible: true,
            shape: Default::default(),
            blinking: false,
        }
    }

    /// A screen whose every cell differs, so misplaced rows show.
    fn screen(revision: u64, columns: u16, rows: u16) -> ScreenSnapshot {
        let cells = (0..usize::from(columns) * usize::from(rows))
            .map(|index| Cell {
                contents: char::from_u32(0x4e00 + (index % 20_000) as u32)
                    .unwrap()
                    .to_string()
                    .into(),
                ..Cell::default()
            })
            .collect();
        ScreenSnapshot::new(revision, TerminalSize { columns, rows }, cells, cursor()).unwrap()
    }

    fn worst_cell() -> Cell {
        let white = Some(CellColor::Rgb(Rgb {
            red: 255,
            green: 255,
            blue: 255,
        }));
        Cell {
            contents: format!("a{}\u{1ab0}", "\u{301}".repeat(14)).into(),
            style: CellStyle::new(white, white, true, true, true, true, true),
            selected: true,
            hyperlink: Some(u16::MAX),
        }
    }

    /// Encode and decode each frame as the wire does, then reassemble.
    fn round_trip(frames: Vec<ServerMessage>) -> (Vec<Assembly>, usize) {
        let mut assembler = ScreenAssembler::default();
        let mut largest = 0;
        let results = frames
            .into_iter()
            .map(|message| {
                let payload = encode_payload(&Envelope {
                    request_id: None,
                    message,
                })
                .unwrap();
                largest = largest.max(payload.len());
                let envelope: Envelope<ServerMessage> = decode_payload(&payload).unwrap();
                assembler.accept(envelope.message)
            })
            .collect();
        assert!(!assembler.in_progress());
        (results, largest)
    }

    fn ready(results: Vec<Assembly>) -> Vec<ServerMessage> {
        results
            .into_iter()
            .filter_map(|result| match result {
                Assembly::Ready(message) => Some(message),
                Assembly::Pending => None,
                Assembly::Discarded(discarded) => panic!("discarded {discarded:?}"),
            })
            .collect()
    }

    #[test]
    fn screens_within_one_frame_are_not_split() {
        let terminal_id = TerminalId::new();
        let fits = ServerMessage::Snapshot {
            terminal_id,
            screen: screen(1, 500, 300),
        };
        assert_eq!(split_screen(fits.clone(), MAX_VISIBLE_CELLS), vec![fits]);
        let other = ServerMessage::Detached;
        assert_eq!(split_screen(other.clone(), 1), vec![other]);
    }

    #[test]
    fn every_screen_kind_reassembles_exactly_from_balanced_chunks() {
        let terminal_id = TerminalId::new();
        let mut full = screen(7, 508, 320);
        full.hyperlinks = vec!["https://example.test/".into()];
        full.cursor.row = 319;
        full.mouse_tracking = true;
        let mut next = full.clone();
        next.revision = 8;
        for cell in &mut next.cells[..508 * 100] {
            cell.contents = "x".into();
        }
        let delta = next
            .delta_from(&full, false)
            .expect("a third of the rows changed");
        assert_eq!(delta.rows.len(), 100);
        let messages = [
            ServerMessage::Snapshot {
                terminal_id,
                screen: full.clone(),
            },
            ServerMessage::CopyModeSnapshot {
                terminal_id,
                screen: full.clone(),
            },
            ServerMessage::CopyModeFinalized {
                terminal_id,
                screen: full.clone(),
            },
            ServerMessage::CopyModeCancelled {
                terminal_id,
                screen: full.clone(),
            },
            ServerMessage::SnapshotDelta {
                terminal_id,
                delta: delta.clone(),
            },
        ];
        for message in messages {
            let delta = matches!(message, ServerMessage::SnapshotDelta { .. });
            for max_cells in [MAX_VISIBLE_CELLS, 20_000, 508, 1] {
                let frames = split_screen(message.clone(), max_cells);
                let rows: Vec<usize> = frames
                    .iter()
                    .filter_map(|frame| match frame {
                        ServerMessage::ScreenRows { rows, .. } => Some(rows.len()),
                        _ => None,
                    })
                    .collect();
                match (delta, max_cells) {
                    // 50,800 cells fit one frame.
                    (true, MAX_VISIBLE_CELLS) => assert_eq!(frames, vec![message.clone()]),
                    // 162,560 cells: two even chunks.
                    (false, MAX_VISIBLE_CELLS) => assert_eq!(rows, [160, 160]),
                    (true, 20_000) => assert_eq!(rows, [34, 34, 32]),
                    (_, max_cells) => {
                        assert!(rows.iter().all(|&rows| rows * 508 <= max_cells.max(508)));
                    }
                }
                let (results, largest) = round_trip(frames);
                assert!(largest < MAX_FRAME_LEN);
                assert_eq!(ready(results), vec![message.clone()], "{max_cells}");
            }
        }
        // The reassembled delta applies like the original.
        let mut applied = full;
        applied.apply_delta(delta).unwrap();
        assert_eq!(applied, next);
    }

    #[test]
    fn worst_case_chunks_and_widest_rows_fit_one_frame() {
        let terminal_id = TerminalId::new();
        // A worst-case grid twice the single-frame bound, with the largest
        // hyperlink table and every placement in its header.
        let size = TerminalSize {
            columns: 1000,
            rows: 300,
        };
        let mut worst = ScreenSnapshot::new(
            u64::MAX,
            size,
            vec![worst_cell(); size.cell_count().unwrap()],
            cursor(),
        )
        .unwrap();
        assert_eq!(worst.cells[0].contents.len(), MAX_CELL_CONTENT_BYTES);
        let entries = usize::from(u16::MAX) + 1;
        let extra = MAX_SCREEN_HYPERLINK_BYTES - entries;
        worst.hyperlinks = vec!["h".into(); entries];
        for uri in &mut worst.hyperlinks[..extra / 31] {
            *uri = "h".repeat(32).into();
        }
        worst.graphics = KittyGraphics {
            images: (0..MAX_KITTY_PLACEMENTS)
                .map(|index| KittyImage {
                    id: u32::MAX - index as u32,
                    generation: u64::MAX,
                    png: std::sync::Arc::from([]),
                })
                .collect(),
            placements: (0..MAX_KITTY_PLACEMENTS)
                .map(|index| KittyPlacement {
                    image_id: u32::MAX - index as u32,
                    placement_id: u32::MAX,
                    column: i32::MIN,
                    row: i32::MIN,
                    columns: u32::MAX,
                    rows: u32::MAX,
                    source_x: u32::MAX,
                    source_y: u32::MAX,
                    source_width: u32::MAX,
                    source_height: u32::MAX,
                    z: i32::MIN,
                })
                .collect(),
        };
        let message = ServerMessage::CopyModeSnapshot {
            terminal_id,
            screen: worst,
        };
        let frames = split_screen(message.clone(), MAX_VISIBLE_CELLS);
        assert_eq!(frames.len(), 2 + 2);
        let (results, largest) = round_trip(frames);
        assert!(largest < MAX_FRAME_LEN, "largest chunk frame {largest}");
        eprintln!("worst-case chunk frame {largest} of {MAX_FRAME_LEN} bytes");
        assert_eq!(ready(results), vec![message]);

        // One row as wide as a terminal can be still fits a frame alone.
        let size = TerminalSize {
            columns: MAX_TERMINAL_COLUMNS,
            rows: 2,
        };
        let wide = ScreenSnapshot::new(
            1,
            size,
            vec![worst_cell(); size.cell_count().unwrap()],
            cursor(),
        )
        .unwrap();
        let message = ServerMessage::Snapshot {
            terminal_id,
            screen: wide,
        };
        let frames = split_screen(message.clone(), MAX_VISIBLE_CELLS / 2);
        assert_eq!(frames.len(), 2 + 2, "one row per chunk");
        let (results, largest) = round_trip(frames);
        assert!(largest < MAX_FRAME_LEN, "widest row frame {largest}");
        eprintln!("widest worst-case row frame {largest} of {MAX_FRAME_LEN} bytes");
        assert_eq!(ready(results), vec![message]);
    }

    fn frames_of(terminal_id: TerminalId, revision: u64, max_cells: usize) -> Vec<ServerMessage> {
        split_screen(
            ServerMessage::Snapshot {
                terminal_id,
                screen: screen(revision, 10, 6),
            },
            max_cells,
        )
    }

    #[test]
    fn a_newer_screen_supersedes_a_partial_one_and_other_interruptions_are_reported() {
        let a = TerminalId::new();
        let b = TerminalId::new();
        let mut assembler = ScreenAssembler::default();
        let first = frames_of(a, 1, 20);
        let second = frames_of(a, 2, 20);
        assert_eq!(first.len(), 2 + 3);
        // Revision 1 stops after its first rows; revision 2 begins.
        for frame in first[..2].iter().cloned() {
            assert!(matches!(assembler.accept(frame), Assembly::Pending));
        }
        let mut completed = Vec::new();
        for frame in second.iter().cloned() {
            match assembler.accept(frame) {
                Assembly::Pending => {}
                Assembly::Ready(message) => completed.push(message),
                Assembly::Discarded(discarded) => panic!("discarded {discarded:?}"),
            }
        }
        assert_eq!(
            completed,
            vec![ServerMessage::Snapshot {
                terminal_id: a,
                screen: screen(2, 10, 6),
            }]
        );
        // Revision 1's leftover rows and end were for a screen no longer in
        // progress: reported once, then ignored.
        assert!(matches!(
            assembler.accept(first[2].clone()),
            Assembly::Discarded(discarded) if discarded == [(a, ChunkError::Unexpected)]
        ));
        for frame in first[3..].iter().cloned() {
            assert!(matches!(assembler.accept(frame), Assembly::Pending));
        }

        // Another terminal's screen beginning loses the unfinished one.
        let third = frames_of(a, 3, 20);
        assert!(matches!(
            assembler.accept(third[0].clone()),
            Assembly::Pending
        ));
        let other = frames_of(b, 9, 20);
        assert!(matches!(
            assembler.accept(other[0].clone()),
            Assembly::Discarded(discarded) if discarded == [(a, ChunkError::Interrupted)]
        ));
        let completed: Vec<_> = other[1..]
            .iter()
            .cloned()
            .filter_map(|frame| match assembler.accept(frame) {
                Assembly::Ready(message) => Some(message),
                _ => None,
            })
            .collect();
        assert_eq!(completed.len(), 1);
        assert!(!assembler.in_progress());
    }

    #[test]
    fn malformed_sequences_are_discarded_once_and_the_next_screen_still_assembles() {
        let terminal_id = TerminalId::new();
        let valid = frames_of(terminal_id, 5, 20);
        let rows = |frame: &ServerMessage| match frame {
            ServerMessage::ScreenRows { rows, .. } => rows.clone(),
            _ => unreachable!(),
        };
        let with_rows = |rows| ServerMessage::ScreenRows {
            terminal_id,
            revision: 5,
            rows,
        };
        let mut swapped = rows(&valid[1]);
        swapped.swap(0, 1);
        let mut narrow = rows(&valid[1]);
        narrow[0].cells.pop();
        let mut header_with_cells = valid[0].clone();
        if let ServerMessage::ScreenBegin { begin, .. } = &mut header_with_cells
            && let ChunkedScreen::Snapshot(screen) = &mut begin.screen
        {
            screen.cells.push(Cell::default());
        }
        let mut huge = valid[0].clone();
        if let ServerMessage::ScreenBegin { begin, .. } = &mut huge
            && let ChunkedScreen::Snapshot(screen) = &mut begin.screen
        {
            screen.size = TerminalSize {
                columns: u16::MAX,
                rows: u16::MAX,
            };
            begin.rows = u16::MAX;
        }
        let cases: Vec<(Vec<ServerMessage>, ChunkError)> = vec![
            (
                vec![valid[0].clone(), with_rows(swapped)],
                ChunkError::RowOrder { row: 1 },
            ),
            (
                vec![valid[0].clone(), with_rows(narrow)],
                ChunkError::RowWidth {
                    row: 0,
                    expected: 10,
                    actual: 9,
                },
            ),
            (
                vec![
                    valid[0].clone(),
                    valid[1].clone(),
                    valid[2].clone(),
                    valid[3].clone(),
                    valid[1].clone(),
                ],
                ChunkError::TooManyRows { expected: 6 },
            ),
            (
                vec![valid[0].clone(), valid[1].clone(), valid[4].clone()],
                ChunkError::Incomplete {
                    expected: 6,
                    actual: 2,
                },
            ),
            (vec![header_with_cells], ChunkError::InvalidHeader),
            (
                vec![huge],
                ChunkError::InvalidSize(TerminalSizeError::TooLarge {
                    actual: usize::from(u16::MAX) * usize::from(u16::MAX),
                    maximum: crate::domain::MAX_GRID_CELLS,
                }),
            ),
            (vec![valid[1].clone()], ChunkError::Unexpected),
        ];
        for (frames, expected) in cases {
            let mut assembler = ScreenAssembler::default();
            let mut reported = Vec::new();
            for frame in frames {
                match assembler.accept(frame) {
                    Assembly::Ready(message) => panic!("assembled {message:?}"),
                    Assembly::Pending => {}
                    Assembly::Discarded(discarded) => reported.extend(discarded),
                }
            }
            assert_eq!(reported, [(terminal_id, expected.clone())]);
            // Whatever is left of a discarded sequence is ignored.
            if !matches!(expected, ChunkError::Incomplete { .. }) {
                for frame in valid[2..].iter().cloned() {
                    assert!(
                        matches!(assembler.accept(frame), Assembly::Pending),
                        "{expected:?}"
                    );
                }
            }
            let next = frames_of(terminal_id, 6, 20);
            let completed: Vec<_> = next
                .into_iter()
                .filter_map(|frame| match assembler.accept(frame) {
                    Assembly::Ready(message) => Some(message),
                    Assembly::Pending => None,
                    Assembly::Discarded(discarded) => panic!("discarded {discarded:?}"),
                })
                .collect();
            assert_eq!(completed.len(), 1, "{expected:?}");
        }
        // An end with no screen in progress.
        assert!(matches!(
            ScreenAssembler::default().accept(valid[4].clone()),
            Assembly::Discarded(discarded) if discarded == [(terminal_id, ChunkError::Unexpected)]
        ));
    }

    #[test]
    fn ordinary_messages_pass_through_untouched() {
        let mut assembler = ScreenAssembler::default();
        let message = ServerMessage::Pong { daemon_pid: 7 };
        assert!(matches!(
            assembler.accept(message.clone()),
            Assembly::Ready(ready) if ready == message
        ));
    }
}
