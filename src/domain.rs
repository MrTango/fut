//! Fut-owned terminal state exchanged between the daemon and its clients.
//!
//! These types deliberately contain no PTY, terminal-emulator, transport, or
//! presentation-library types.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use compact_str::CompactString;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use unicode_segmentation::UnicodeSegmentation;
use uuid::Uuid;

/// Length of Fut's complete, URL-safe representation of a 128-bit resource ID.
pub const COMPACT_ID_LEN: usize = 23;

#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[error("expected a UUID or {COMPACT_ID_LEN}-character compact Fut ID")]
pub struct IdParseError;

#[must_use]
pub fn compact_id(id: Uuid) -> String {
    format!("f{}", URL_SAFE_NO_PAD.encode(id.as_bytes()))
}

pub fn parse_id(value: &str) -> Result<Uuid, IdParseError> {
    if let Ok(id) = Uuid::parse_str(value) {
        return Ok(id);
    }
    if value.len() != COMPACT_ID_LEN {
        return Err(IdParseError);
    }
    let encoded = value.strip_prefix('f').ok_or(IdParseError)?;
    let bytes = URL_SAFE_NO_PAD.decode(encoded).map_err(|_| IdParseError)?;
    let bytes: [u8; 16] = bytes.try_into().map_err(|_| IdParseError)?;
    let id = Uuid::from_bytes(bytes);
    (compact_id(id) == value).then_some(id).ok_or(IdParseError)
}

/// Upper bound for the number of cells in a terminal's visible grid.
///
/// This is deliberately independent of the transport frame limit: validating
/// dimensions before constructing a grid prevents hostile `u16` dimensions
/// from driving multi-gigabyte allocations. The bound is also sized so the
/// worst-case screen still fits one frame: at most about 50 bytes per fully
/// styled, selected, hyperlinked cell, plus [`MAX_SCREEN_HYPERLINK_BYTES`]
/// and [`MAX_KITTY_PLACEMENTS`] worth of image references. Kitty image
/// pixels travel in their own frames, so they do not share this budget.
pub const MAX_VISIBLE_CELLS: usize = 150_000;
/// Visible-cell bound for remote peers that did not negotiate
/// `large-screens.v1`. Generation-1 screens embed Kitty image pixels and were
/// frozen with this bound, so those peers must never see a larger grid.
pub const LEGACY_MAX_VISIBLE_CELLS: usize = 50_000;
/// Maximum Kitty placements carried in one screen. Bounds the image
/// references and placements that share a screen's frame with its cells.
pub const MAX_KITTY_PLACEMENTS: usize = 1024;
/// Maximum UTF-8 bytes retained for one serialized terminal cell.
///
/// Cell contents are also constrained to one control-free grapheme, and the
/// wire format (MessagePack) stores strings as raw length-prefixed bytes
/// with no escaping overhead. At 32 bytes, [`MAX_VISIBLE_CELLS`] fully
/// styled, selected, hyperlinked cells still fit below the protocol's
/// eight-MiB limit, while ordinary emoji clusters remain exact. Pathological
/// combining sequences are represented by a fixed marker.
pub const MAX_CELL_CONTENT_BYTES: usize = 32;
pub const OVERSIZED_CELL_CONTENT_MARKER: &str = "�";
/// Maximum UTF-8 bytes retained for one explicit OSC 8 hyperlink URI.
pub const MAX_HYPERLINK_URI_BYTES: usize = 4 * 1024;
/// Maximum unique hyperlink URI bytes retained in one visible snapshot.
pub const MAX_SCREEN_HYPERLINK_BYTES: usize = 512 * 1024;
/// Maximum plain-text payload returned by a copy-mode request. At one MiB,
/// this remains well below the eight-MiB frame limit.
pub const MAX_COPY_BYTES: usize = 1024 * 1024;
/// Maximum physical cells traversed while formatting one selection.
///
/// This is deliberately separate from [`MAX_COPY_BYTES`]: a large blank
/// selection can produce very little text while still requiring substantial
/// emulator work.
pub const MAX_COPY_CELLS: usize = 250_000;
/// Maximum literal query accepted by the bounded on-demand scrollback scan.
pub const MAX_SEARCH_QUERY_BYTES: usize = 4 * 1024;
/// Maximum physical rows selected by terminal output inspection.
pub const MAX_TERMINAL_OUTPUT_ROWS: usize = 2_000;
/// Maximum cells traversed by one terminal output inspection.
pub const MAX_TERMINAL_OUTPUT_CELLS: usize = 250_000;
/// Maximum UTF-8/ANSI bytes returned by terminal output inspection.
pub const MAX_TERMINAL_OUTPUT_BYTES: usize = 1024 * 1024;
/// Maximum literal or regular-expression pattern accepted by output waits.
pub const MAX_TERMINAL_OUTPUT_PATTERN_BYTES: usize = 4 * 1024;
/// Maximum number of terminal cells inspected by one literal search. The
/// search map uses three `u32` values per retained cell, bounding its primary
/// temporary allocation to roughly three MiB.
pub const MAX_SEARCH_CELLS: usize = 250_000;
/// Maximum UTF-8 search material assembled from those cells.
pub const MAX_SEARCH_TEXT_BYTES: usize = 2 * 1024 * 1024;
/// Maximum Unicode scalar values accepted from one terminal cell while
/// building search text. This bounds the temporary `Vec<char>` independently
/// of the UTF-8 text cap.
pub const MAX_SEARCH_CELL_CODEPOINTS: usize = 4 * 1024;

macro_rules! id_type {
    ($name:ident) => {
        #[derive(
            Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            /// The canonical UUID retained on the wire and in structured output.
            #[must_use]
            pub fn uuid(self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(&compact_id(self.0))
            }
        }

        impl std::str::FromStr for $name {
            type Err = IdParseError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                parse_id(value).map(Self)
            }
        }
    };
}

id_type!(TerminalId);
id_type!(ClientId);
id_type!(SessionId);
id_type!(WorkspaceId);
id_type!(TabId);
id_type!(PaneId);
id_type!(SplitId);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CopyModeMovement {
    Left,
    Down,
    Up,
    Right,
    BeginningOfLine,
    EndOfLine,
    PageUp,
    PageDown,
}

/// A logical key whose bytes must be chosen from terminal-owned keyboard modes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalKeyCode {
    Character(char),
    Enter,
    Tab,
    Backspace,
    Escape,
    Up,
    Down,
    Right,
    Left,
    Home,
    End,
    Insert,
    Delete,
    PageUp,
    PageDown,
    Function(u8),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct TerminalKeyModifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub super_key: bool,
    pub caps_lock: bool,
    pub num_lock: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalKeyAction {
    #[default]
    Press,
    Repeat,
    Release,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TerminalKeyEvent {
    pub code: TerminalKeyCode,
    #[serde(default)]
    pub modifiers: TerminalKeyModifiers,
    #[serde(default)]
    pub action: TerminalKeyAction,
    /// Text generated by the host keyboard layout before Ctrl/Alt transforms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchDirection {
    Forward,
    Backward,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CopyModeAction {
    Begin,
    BeginSelection { column: u16, row: u16 },
    SetSelectionEnd { column: u16, row: u16 },
    Move { movement: CopyModeMovement },
    ToggleSelection,
    Search { query: String },
    RepeatSearch { direction: SearchDirection },
    Copy,
    FinalizeCopy { copy_id: Uuid },
    Cancel,
}

impl CopyModeAction {
    pub(crate) fn begins(&self) -> bool {
        matches!(self, Self::Begin | Self::BeginSelection { .. })
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CopyModeError {
    #[error("copy mode is not active")]
    NotActive,
    #[error("copy mode is already active")]
    AlreadyActive,
    #[error("copy-mode cursor was discarded by the terminal")]
    CursorLost,
    #[error("search query is empty")]
    EmptySearchQuery,
    #[error("search query is {actual} bytes; maximum is {maximum}")]
    SearchQueryTooLarge { actual: usize, maximum: usize },
    #[error("search would inspect {actual} cells; maximum is {maximum}")]
    SearchSpaceTooLarge { actual: usize, maximum: usize },
    #[error("search text is at least {actual} bytes; maximum is {maximum}")]
    SearchTextTooLarge { actual: usize, maximum: usize },
    #[error("there is no previous search")]
    NoSearch,
    #[error("literal text was not found")]
    NoMatch,
    #[error("search cell has {actual} codepoints; maximum is {maximum}")]
    SearchCellTooLarge { actual: usize, maximum: usize },
    #[error(
        "copy-mode position ({column}, {row}) is outside the {columns}x{rows} terminal viewport"
    )]
    PositionOutOfBounds {
        column: u16,
        row: u16,
        columns: u16,
        rows: u16,
    },
    #[error("selection spans {actual} cells; maximum is {maximum}")]
    CopySpanTooLarge { actual: usize, maximum: usize },
    #[error("selected text is {actual} bytes; maximum is {maximum}")]
    CopyTooLarge { actual: usize, maximum: usize },
    #[error("copy confirmation does not match the prepared selection")]
    CopyConfirmationMismatch,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct MouseModifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseWheelDirection {
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    Left,
    Middle,
    Right,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct MouseButtons {
    pub left: bool,
    pub middle: bool,
    pub right: bool,
}

impl MouseButtons {
    #[must_use]
    pub fn contains(self, button: MouseButton) -> bool {
        match button {
            MouseButton::Left => self.left,
            MouseButton::Middle => self.middle,
            MouseButton::Right => self.right,
        }
    }

    pub fn set(&mut self, button: MouseButton, pressed: bool) {
        match button {
            MouseButton::Left => self.left = pressed,
            MouseButton::Middle => self.middle = pressed,
            MouseButton::Right => self.right = pressed,
        }
    }

    #[must_use]
    pub fn any(self) -> bool {
        self.left || self.middle || self.right
    }
}

impl MouseButton {
    pub const ALL: [Self; 3] = [Self::Left, Self::Middle, Self::Right];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MouseEventKind {
    Press {
        button: MouseButton,
    },
    Release {
        button: MouseButton,
    },
    Motion {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        button: Option<MouseButton>,
    },
    Wheel {
        direction: MouseWheelDirection,
    },
}

/// Mouse input normalized to zero-based terminal cell coordinates.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MouseEvent {
    pub kind: MouseEventKind,
    pub column: u16,
    pub row: u16,
    #[serde(default)]
    pub modifiers: MouseModifiers,
    /// Button state after this event. Ghostty uses this to distinguish a
    /// requested drag from unbuttoned motion.
    #[serde(default)]
    pub buttons: MouseButtons,
}

impl MouseEvent {
    /// Validate the post-event button snapshot supplied by an untrusted client.
    pub fn validate(self) -> Result<(), &'static str> {
        match self.kind {
            MouseEventKind::Press { button } if !self.buttons.contains(button) => {
                Err("pressed button is absent from the post-event button snapshot")
            }
            MouseEventKind::Release { button } if self.buttons.contains(button) => {
                Err("released button remains in the post-event button snapshot")
            }
            MouseEventKind::Motion {
                button: Some(button),
            } if !self.buttons.contains(button) => {
                Err("drag button is absent from the button snapshot")
            }
            _ => Ok(()),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusState {
    #[default]
    Idle,
    Working,
    Done,
    Blocked,
    Error,
}

impl StatusState {
    fn is_active(self) -> bool {
        matches!(self, Self::Working | Self::Blocked)
    }

    fn is_result(self) -> bool {
        matches!(self, Self::Done | Self::Error)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockedKind {
    Permission,
    Question,
    Auth,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StatusSource {
    Osc,
    Report {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_session_id: Option<String>,
    },
    Detection {
        agent: String,
        rule: String,
    },
}

impl StatusSource {
    /// Weak provenance for root records inferred from legacy ConEmu-style
    /// OSC 9;4 progress. It reuses the detection shape so the frozen remote
    /// contract needs no new enum case, and so it inherits detection's
    /// precedence: explicit OSC 7501 and CLI reports always win.
    #[must_use]
    pub fn legacy_progress() -> Self {
        Self::Detection {
            agent: "progress".into(),
            rule: "osc_9_4".into(),
        }
    }

    /// OSC 9;4 does not say which program is running, so it names no app.
    fn is_legacy_progress(&self) -> bool {
        matches!(self, Self::Detection { agent, rule } if agent == "progress" && rule == "osc_9_4")
    }

    fn is_explicit(&self) -> bool {
        !matches!(self, Self::Detection { .. })
    }
}

pub const MAX_STATUS_ID_BYTES: usize = 128;
pub const MAX_STATUS_ID_SEGMENT_BYTES: usize = 32;
pub const MAX_STATUS_ID_LEVELS: usize = 8;
pub const MAX_STATUS_APP_BYTES: usize = 32;
pub const MAX_STATUS_TITLE_BYTES: usize = 192;
pub const MAX_STATUS_MESSAGE_BYTES: usize = 2048;
pub const MAX_STATUS_METADATA_VALUE_BYTES: usize = 128;
pub const MAX_STATUS_RECORDS: usize = 256;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StatusReport {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    pub state: StatusState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<BlockedKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl StatusReport {
    pub fn validate(&self) -> Result<(), &'static str> {
        validate_status_id(&self.id)?;
        if self.progress.is_some_and(|progress| progress > 100) {
            return Err("status progress must be between 0 and 100");
        }
        if let Some(app) = &self.app
            && (app.is_empty()
                || app.len() > MAX_STATUS_APP_BYTES
                || !app.bytes().all(is_status_identifier_byte))
        {
            return Err("invalid status app");
        }
        validate_status_text(
            self.title.as_deref(),
            MAX_STATUS_TITLE_BYTES,
            "status title",
        )?;
        validate_status_text(
            self.message.as_deref(),
            MAX_STATUS_MESSAGE_BYTES,
            "status message",
        )?;
        Ok(())
    }

    fn normalized(mut self) -> Self {
        if self.state != StatusState::Blocked {
            self.kind = None;
        }
        if !self.state.is_active() {
            self.progress = None;
        }
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum StatusUpdate {
    Report(StatusReport),
    Clear {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    PromptStart,
    ProcessExit,
}

impl StatusUpdate {
    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::Report(report) => report.validate(),
            Self::Clear { id: Some(id) } => validate_status_id(id),
            Self::Clear { id: None } | Self::PromptStart | Self::ProcessExit => Ok(()),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StatusRecord {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    pub state: StatusState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<BlockedKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub revision: u64,
    pub updated_at_ms: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub attention_revision: u64,
}

impl StatusRecord {
    fn report_eq(&self, report: &StatusReport) -> bool {
        self.id == report.id
            && self.state == report.state
            && self.kind == report.kind
            && self.progress == report.progress
            && self.app == report.app
            && self.title == report.title
            && self.message == report.message
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProgramStatus {
    pub records: Vec<StatusRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<StatusSource>,
    pub revision: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub attention_revision: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub read_revision: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub working_revision: u64,
}

/// Private status state that must survive independently from the public wire
/// representation. Record provenance determines reporting eligibility while
/// active explicit IDs form the short-lived lease that suppresses detection.
#[derive(Clone, Debug)]
struct StatusOrigin {
    source: StatusSource,
    automatic_pid: Option<u32>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct StatusBookkeeping {
    lru: Vec<String>,
    origins: BTreeMap<String, StatusOrigin>,
    active_explicit_ids: BTreeSet<String>,
}

impl ProgramStatus {
    #[must_use]
    pub fn root(&self) -> Option<&StatusRecord> {
        self.records.iter().find(|record| record.id.is_empty())
    }

    #[must_use]
    pub fn summary_record(&self) -> Option<&StatusRecord> {
        self.records.iter().max_by_key(|record| {
            let priority = match record.state {
                StatusState::Blocked => 4,
                StatusState::Error => 3,
                StatusState::Working => 2,
                StatusState::Done => 1,
                StatusState::Idle => 0,
            };
            (priority, record.revision, record.updated_at_ms)
        })
    }

    #[must_use]
    pub fn summary_state(&self) -> StatusState {
        self.summary_record()
            .map_or(StatusState::Idle, |record| record.state)
    }

    #[must_use]
    pub fn progress(&self) -> Option<u8> {
        if let Some(progress) = self
            .root()
            .filter(|record| record.state.is_active())
            .and_then(|record| record.progress)
        {
            return Some(progress);
        }

        let mut active = self
            .records
            .iter()
            .filter(|record| record.state.is_active());
        let record = active.next()?;
        if active.next().is_some() {
            return None;
        }
        record.progress
    }

    #[must_use]
    pub fn is_working(&self) -> bool {
        self.records
            .iter()
            .any(|record| record.state == StatusState::Working)
    }

    /// Records whose state can demand attention. A done result is deferred
    /// while any record is still working: a root that finished its turn with
    /// sub-tasks running has not completed as a whole, so its completion
    /// surfaces once the last working record settles or clears.
    pub fn attention_candidates(&self) -> impl Iterator<Item = &StatusRecord> {
        let working = self.is_working();
        self.records
            .iter()
            .filter(move |record| match record.state {
                StatusState::Blocked | StatusState::Error => true,
                StatusState::Done => !working,
                StatusState::Idle | StatusState::Working => false,
            })
    }

    #[must_use]
    pub fn attention(&self) -> Option<&StatusRecord> {
        self.attention_candidates()
            .filter(|record| record.attention_revision != 0)
            .max_by_key(|record| record.attention_revision)
    }

    #[must_use]
    pub fn has_unread_attention(&self) -> bool {
        self.attention()
            .is_some_and(|record| record.attention_revision > self.read_revision)
    }

    #[must_use]
    pub fn is_reporting(&self) -> bool {
        !self.records.is_empty() && self.source.as_ref().is_some_and(StatusSource::is_explicit)
    }

    #[must_use]
    pub fn app<'a>(&'a self, record: &'a StatusRecord) -> Option<&'a str> {
        if let Some(app) = record.app.as_deref() {
            return Some(app);
        }
        let mut id = record.id.as_str();
        loop {
            id = id.rsplit_once('/').map_or("", |(parent, _)| parent);
            if let Some(app) = self
                .records
                .iter()
                .find(|candidate| candidate.id == id)
                .and_then(|candidate| candidate.app.as_deref())
            {
                return Some(app);
            }
            if id.is_empty() {
                return None;
            }
        }
    }

    #[must_use]
    pub fn label(&self) -> Option<&str> {
        self.summary_record()
            .and_then(|record| self.app(record))
            .or(match self.source.as_ref() {
                Some(StatusSource::Report { source, .. }) => {
                    source.as_deref().filter(|source| !source.is_empty())
                }
                Some(source @ StatusSource::Detection { agent, .. })
                    if !source.is_legacy_progress() =>
                {
                    Some(agent.as_str())
                }
                Some(StatusSource::Osc | StatusSource::Detection { .. }) | None => None,
            })
    }

    #[must_use]
    pub fn updated_at_ms(&self) -> u64 {
        self.records
            .iter()
            .map(|record| record.updated_at_ms)
            .max()
            .unwrap_or(0)
    }

    pub fn validate_source(source: &StatusSource) -> Result<(), &'static str> {
        let values: &[&str] = match source {
            StatusSource::Osc => &[],
            StatusSource::Report {
                source,
                agent_session_id,
            } => &[
                source.as_deref().unwrap_or_default(),
                agent_session_id.as_deref().unwrap_or_default(),
            ],
            StatusSource::Detection { agent, rule } => &[agent, rule],
        };
        if values
            .iter()
            .any(|value| value.len() > MAX_STATUS_METADATA_VALUE_BYTES || contains_control(value))
        {
            return Err("invalid status source metadata");
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn apply_updates(
        &mut self,
        updates: Vec<StatusUpdate>,
        source: StatusSource,
        revision: u64,
        now_ms: u64,
    ) -> bool {
        let mut bookkeeping = StatusBookkeeping::from_status(self);
        self.apply_updates_with_bookkeeping(
            updates,
            Some(source),
            None,
            revision,
            now_ms,
            &mut bookkeeping,
        )
    }

    pub(crate) fn apply_updates_with_bookkeeping(
        &mut self,
        updates: Vec<StatusUpdate>,
        source: Option<StatusSource>,
        automatic_pid: Option<u32>,
        revision: u64,
        now_ms: u64,
        bookkeeping: &mut StatusBookkeeping,
    ) -> bool {
        bookkeeping.sync(self);

        let detection = source
            .as_ref()
            .is_some_and(|source| matches!(source, StatusSource::Detection { .. }));
        if detection && bookkeeping.has_active_explicit() {
            return false;
        }
        let mut changed = false;
        let mut preferred_source = None;
        for update in updates {
            match update {
                StatusUpdate::Report(mut report) => {
                    let source = source
                        .as_ref()
                        .expect("status reports always have a source");
                    if !detection {
                        changed |= self.remove_detection_records(bookkeeping);
                    }
                    if detection && report.state == StatusState::Idle {
                        match self
                            .records
                            .iter()
                            .find(|record| record.id == report.id)
                            .map(|record| record.state)
                        {
                            Some(StatusState::Working) => report.state = StatusState::Done,
                            Some(StatusState::Done | StatusState::Error) => continue,
                            Some(StatusState::Idle | StatusState::Blocked) | None => {}
                        }
                    }
                    let id = report.id.clone();
                    changed |= self.replace_record(
                        report.normalized(),
                        revision,
                        now_ms,
                        &mut bookkeeping.lru,
                    );
                    bookkeeping.origins.insert(
                        id.clone(),
                        StatusOrigin {
                            source: source.clone(),
                            automatic_pid,
                        },
                    );
                    if !detection {
                        bookkeeping.active_explicit_ids.insert(id);
                    }
                    preferred_source = Some(source.clone());
                    bookkeeping.sync(self);
                }
                StatusUpdate::Clear { id } => {
                    // A weak source only retracts what it inferred itself.
                    let removed = match source.as_ref().filter(|_| detection) {
                        Some(source) => {
                            self.remove_inferred_subtree(id.as_deref(), source, bookkeeping)
                        }
                        None => self.remove_subtree(id.as_deref(), &mut bookkeeping.lru),
                    };
                    changed |= removed;
                    bookkeeping.sync(self);
                }
                StatusUpdate::PromptStart | StatusUpdate::ProcessExit => {
                    let before = self.records.len();
                    self.records.retain(|record| record.state.is_result());
                    if self.records.len() != before {
                        changed = true;
                    }
                    bookkeeping.active_explicit_ids.clear();
                    for origin in bookkeeping.origins.values_mut() {
                        origin.automatic_pid = None;
                    }
                    bookkeeping.sync(self);
                }
            }
        }

        let source = bookkeeping.public_source(self, preferred_source.as_ref());
        if self.source != source {
            self.source = source;
            changed = true;
        }
        if changed {
            self.revision = revision;
        }
        changed
    }

    pub(crate) fn release_automatic_pid(
        &mut self,
        automatic_pid: u32,
        revision: u64,
        bookkeeping: &mut StatusBookkeeping,
    ) -> bool {
        bookkeeping.sync(self);
        let owned_ids = bookkeeping
            .origins
            .iter()
            .filter_map(|(id, origin)| {
                (origin.automatic_pid == Some(automatic_pid)
                    && bookkeeping.owner_releases(id, origin))
                .then_some(id.clone())
            })
            .collect::<BTreeSet<_>>();
        if owned_ids.is_empty() {
            return false;
        }

        let before = self.records.len();
        self.records
            .retain(|record| !owned_ids.contains(&record.id) || record.state.is_result());
        let mut changed = self.records.len() != before;
        for id in owned_ids {
            bookkeeping.active_explicit_ids.remove(&id);
            if let Some(origin) = bookkeeping.origins.get_mut(&id) {
                origin.automatic_pid = None;
            }
        }
        bookkeeping.sync(self);

        let source = bookkeeping.public_source(self, None);
        if self.source != source {
            self.source = source;
            changed = true;
        }
        if changed {
            self.revision = revision;
        }
        changed
    }

    fn remove_detection_records(&mut self, bookkeeping: &mut StatusBookkeeping) -> bool {
        let detection_ids = bookkeeping
            .origins
            .iter()
            .filter_map(|(id, origin)| (!origin.source.is_explicit()).then_some(id.clone()))
            .collect::<BTreeSet<_>>();
        if detection_ids.is_empty() {
            return false;
        }
        let before = self.records.len();
        self.records
            .retain(|record| !detection_ids.contains(&record.id));
        let changed = self.records.len() != before;
        if changed {
            bookkeeping.sync(self);
        }
        changed
    }

    fn replace_record(
        &mut self,
        report: StatusReport,
        revision: u64,
        now_ms: u64,
        lru: &mut Vec<String>,
    ) -> bool {
        let existing_index = self
            .records
            .iter()
            .position(|record| record.id == report.id);
        self.touch_lru(&report.id, lru);
        if existing_index.is_some_and(|index| self.records[index].report_eq(&report)) {
            return false;
        }

        let existing = existing_index.map(|index| self.records.remove(index));
        let is_new = existing_index.is_none();
        let previous_state = existing.as_ref().map(|record| record.state);
        let attention_revision = if matches!(
            report.state,
            StatusState::Blocked | StatusState::Done | StatusState::Error
        ) {
            match existing.as_ref() {
                Some(record)
                    if record.state == report.state
                        && !(report.state == StatusState::Blocked
                            && (record.kind != report.kind
                                || record.title != report.title
                                || record.message != report.message)) =>
                {
                    record.attention_revision
                }
                Some(_) | None => revision,
            }
        } else {
            0
        };
        if attention_revision == revision {
            self.attention_revision = revision;
        }
        if report.state == StatusState::Working && previous_state != Some(StatusState::Working) {
            self.working_revision = revision;
        }
        let record = StatusRecord {
            id: report.id,
            state: report.state,
            kind: report.kind,
            progress: report.progress,
            app: report.app,
            title: report.title,
            message: report.message,
            revision,
            updated_at_ms: now_ms,
            attention_revision,
        };
        if record.id.is_empty() {
            self.records.insert(0, record);
        } else if let Some(index) = existing_index {
            self.records.insert(index, record);
        } else {
            self.records.push(record);
        }
        if is_new && self.records.len() > MAX_STATUS_RECORDS {
            let evicted = lru.remove(0);
            let index = self
                .records
                .iter()
                .position(|record| record.id == evicted)
                .expect("LRU record exists");
            self.records.remove(index);
        }
        true
    }

    fn remove_inferred_subtree(
        &mut self,
        id: Option<&str>,
        source: &StatusSource,
        bookkeeping: &mut StatusBookkeeping,
    ) -> bool {
        let target = id.unwrap_or_default();
        let inferred = bookkeeping
            .origins
            .iter()
            .filter(|(record_id, origin)| {
                &origin.source == source && status_subtree_contains(target, record_id)
            })
            .map(|(record_id, _)| record_id.clone())
            .collect::<BTreeSet<_>>();
        let before = self.records.len();
        self.records.retain(|record| !inferred.contains(&record.id));
        let changed = self.records.len() != before;
        if changed {
            bookkeeping.sync(self);
        }
        changed
    }

    fn remove_subtree(&mut self, id: Option<&str>, lru: &mut Vec<String>) -> bool {
        let id = id.unwrap_or_default();
        let before = self.records.len();
        self.records
            .retain(|record| !status_subtree_contains(id, &record.id));
        let changed = self.records.len() != before;
        if changed {
            self.sync_lru(lru);
        }
        changed
    }

    fn sync_lru(&self, lru: &mut Vec<String>) {
        lru.retain(|id| self.records.iter().any(|record| record.id == *id));
        let mut missing = self
            .records
            .iter()
            .enumerate()
            .filter(|(_, record)| !lru.contains(&record.id))
            .collect::<Vec<_>>();
        missing.sort_by_key(|(index, record)| (record.updated_at_ms, record.revision, *index));
        lru.extend(missing.into_iter().map(|(_, record)| record.id.clone()));
    }

    fn touch_lru(&self, id: &str, lru: &mut Vec<String>) {
        lru.retain(|candidate| candidate != id);
        lru.push(id.to_owned());
    }
}

impl StatusBookkeeping {
    #[cfg(test)]
    fn from_status(status: &ProgramStatus) -> Self {
        let origins = status.source.as_ref().map_or_else(BTreeMap::new, |source| {
            status
                .records
                .iter()
                .map(|record| {
                    (
                        record.id.clone(),
                        StatusOrigin {
                            source: source.clone(),
                            automatic_pid: None,
                        },
                    )
                })
                .collect()
        });
        Self {
            lru: status
                .records
                .iter()
                .map(|record| record.id.clone())
                .collect(),
            origins,
            active_explicit_ids: BTreeSet::new(),
        }
    }

    pub(crate) fn has_detection(&self, detector: &str) -> bool {
        self.origins.values().any(|origin| {
            matches!(&origin.source, StatusSource::Detection { agent, .. } if agent == detector)
        })
    }

    /// Whether the record's foreground-process owner releases it on exit:
    /// explicit records while their lease is active, and inferred records
    /// (legacy progress), which have no lease of their own.
    fn owner_releases(&self, id: &str, origin: &StatusOrigin) -> bool {
        self.active_explicit_ids.contains(id) || !origin.source.is_explicit()
    }

    pub(crate) fn automatic_pids(&self) -> BTreeSet<u32> {
        self.origins
            .iter()
            .filter(|(id, origin)| self.owner_releases(id, origin))
            .filter_map(|(_, origin)| origin.automatic_pid)
            .collect()
    }

    fn has_active_explicit(&self) -> bool {
        !self.active_explicit_ids.is_empty()
    }

    fn sync(&mut self, status: &ProgramStatus) {
        status.sync_lru(&mut self.lru);
        self.origins
            .retain(|id, _| status.records.iter().any(|record| record.id == *id));
        self.active_explicit_ids
            .retain(|id| status.records.iter().any(|record| record.id == *id));
        if let Some(source) = status.source.as_ref() {
            for record in &status.records {
                self.origins
                    .entry(record.id.clone())
                    .or_insert_with(|| StatusOrigin {
                        source: source.clone(),
                        automatic_pid: None,
                    });
            }
        }
    }

    fn public_source(
        &self,
        status: &ProgramStatus,
        preferred: Option<&StatusSource>,
    ) -> Option<StatusSource> {
        if status.records.is_empty() {
            return None;
        }
        let has_explicit = self
            .origins
            .values()
            .any(|origin| origin.source.is_explicit());
        let eligible = |origin: &&StatusOrigin| !has_explicit || origin.source.is_explicit();
        if let Some(preferred) = preferred
            && self
                .origins
                .values()
                .any(|origin| eligible(&origin) && &origin.source == preferred)
        {
            return Some(preferred.clone());
        }
        if let Some(current) = status.source.as_ref()
            && self
                .origins
                .values()
                .any(|origin| eligible(&origin) && &origin.source == current)
        {
            return Some(current.clone());
        }
        self.lru
            .iter()
            .rev()
            .filter_map(|id| self.origins.get(id))
            .find(eligible)
            .map(|origin| origin.source.clone())
    }
}

/// Whether `id` lies in the subtree rooted at `target`; the empty target is
/// the whole tree.
fn status_subtree_contains(target: &str, id: &str) -> bool {
    target.is_empty()
        || id == target
        || id
            .strip_prefix(target)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn validate_status_id(id: &str) -> Result<(), &'static str> {
    if id.len() > MAX_STATUS_ID_BYTES {
        return Err("status id is too long");
    }
    if id.is_empty() {
        return Ok(());
    }
    let mut levels = 0;
    for segment in id.split('/') {
        levels += 1;
        if segment.is_empty()
            || segment.len() > MAX_STATUS_ID_SEGMENT_BYTES
            || !segment.bytes().all(is_status_identifier_byte)
        {
            return Err("invalid status id");
        }
    }
    if levels > MAX_STATUS_ID_LEVELS {
        return Err("status id has too many levels");
    }
    Ok(())
}

fn is_status_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"_.+-".contains(&byte)
}

fn validate_status_text(
    value: Option<&str>,
    maximum: usize,
    name: &'static str,
) -> Result<(), &'static str> {
    if value.is_some_and(|value| value.len() > maximum || contains_control(value)) {
        return Err(match name {
            "status title" => "invalid status title",
            _ => "invalid status message",
        });
    }
    Ok(())
}

fn contains_control(value: &str) -> bool {
    value
        .chars()
        .any(|character| matches!(character as u32, 0x00..=0x1f | 0x7f..=0x9f))
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalOutputSource {
    #[default]
    Visible,
    Recent,
    RecentUnwrapped,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum TerminalOutputMatcher {
    Literal(String),
    Regex(String),
}

impl TerminalOutputMatcher {
    #[must_use]
    pub fn value(&self) -> &str {
        match self {
            Self::Literal(value) | Self::Regex(value) => value,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TerminalOutput {
    pub version: u8,
    pub terminal_id: TerminalId,
    pub revision: u64,
    pub source: TerminalOutputSource,
    pub requested_rows: usize,
    pub returned_rows: usize,
    pub truncated: bool,
    pub starts_mid_logical_line: bool,
    pub ansi: bool,
    pub text: String,
}

/// Prompts are deliberately smaller than the local protocol frame so malformed
/// callers cannot monopolize the terminal input queue with one message.
pub const MAX_AGENT_PROMPT_BYTES: usize = 1024 * 1024;

fn is_zero(value: &u64) -> bool {
    *value == 0
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TerminalSize {
    #[serde(rename = "c")]
    pub columns: u16,
    #[serde(rename = "r")]
    pub rows: u16,
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum TerminalSizeError {
    #[error("terminal dimensions must be non-zero")]
    Empty,
    #[error("terminal dimensions require {actual} visible cells; maximum is {maximum}")]
    TooLarge { actual: usize, maximum: usize },
}

impl TerminalSize {
    pub fn validate(self) -> Result<(), TerminalSizeError> {
        self.cell_count().map(|_| ())
    }

    /// Validate against a tighter peer-specific bound such as
    /// [`LEGACY_MAX_VISIBLE_CELLS`].
    pub fn validate_within(self, maximum: usize) -> Result<(), TerminalSizeError> {
        self.cell_count_within(maximum).map(|_| ())
    }

    pub fn cell_count(self) -> Result<usize, TerminalSizeError> {
        self.cell_count_within(MAX_VISIBLE_CELLS)
    }

    fn cell_count_within(self, maximum: usize) -> Result<usize, TerminalSizeError> {
        if self.columns == 0 || self.rows == 0 {
            return Err(TerminalSizeError::Empty);
        }

        let count = usize::from(self.columns) * usize::from(self.rows);
        if count > maximum.min(MAX_VISIBLE_CELLS) {
            return Err(TerminalSizeError::TooLarge {
                actual: count,
                maximum: maximum.min(MAX_VISIBLE_CELLS),
            });
        }
        Ok(count)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Rgb {
    #[serde(rename = "r")]
    pub red: u8,
    #[serde(rename = "g")]
    pub green: u8,
    #[serde(rename = "b")]
    pub blue: u8,
}

/// Host terminal colors, as reported by OSC 10/11 and OSC 4.
/// Missing values remain unknown rather than inventing a terminal theme.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct TerminalColors {
    pub foreground: Option<Rgb>,
    pub background: Option<Rgb>,
    pub palette: [Option<Rgb>; 16],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CellColor {
    Indexed(u8),
    Rgb(Rgb),
}

/// Wire tags packed into a color's top byte, keeping "no color" (0) distinct
/// from `Indexed(0)`.
const COLOR_TAG_INDEXED: u32 = 0x0100_0000;
const COLOR_TAG_RGB: u32 = 0x0200_0000;

const FLAG_BOLD: u8 = 1 << 0;
const FLAG_ITALIC: u8 = 1 << 1;
const FLAG_UNDERLINE: u8 = 1 << 2;
const FLAG_INVERSE: u8 = 1 << 3;
const FLAG_FAINT: u8 = 1 << 4;
const PACKED_COLOR_BITS: u32 = 26;
const PACKED_COLOR_MASK: u64 = (1 << PACKED_COLOR_BITS) - 1;

/// Compact in-memory and wire representation of terminal cell colors and
/// modifiers. Both 26-bit colors and five flags fit losslessly in one word.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CellStyle(u64);

fn pack_color(color: Option<CellColor>) -> u32 {
    match color {
        None => 0,
        Some(CellColor::Indexed(index)) => COLOR_TAG_INDEXED | u32::from(index),
        Some(CellColor::Rgb(Rgb { red, green, blue })) => {
            COLOR_TAG_RGB | (u32::from(red) << 16) | (u32::from(green) << 8) | u32::from(blue)
        }
    }
}

fn unpack_color(packed: u32) -> Option<CellColor> {
    match packed & 0xff00_0000 {
        0 => None,
        COLOR_TAG_INDEXED => Some(CellColor::Indexed(packed as u8)),
        _ => Some(CellColor::Rgb(Rgb {
            red: (packed >> 16) as u8,
            green: (packed >> 8) as u8,
            blue: packed as u8,
        })),
    }
}

impl CellStyle {
    #[must_use]
    pub fn new(
        foreground: Option<CellColor>,
        background: Option<CellColor>,
        bold: bool,
        italic: bool,
        underline: bool,
        inverse: bool,
        faint: bool,
    ) -> Self {
        let mut flags = 0;
        if bold {
            flags |= FLAG_BOLD;
        }
        if italic {
            flags |= FLAG_ITALIC;
        }
        if underline {
            flags |= FLAG_UNDERLINE;
        }
        if inverse {
            flags |= FLAG_INVERSE;
        }
        if faint {
            flags |= FLAG_FAINT;
        }
        Self(
            u64::from(pack_color(foreground))
                | (u64::from(pack_color(background)) << PACKED_COLOR_BITS)
                | (u64::from(flags) << (PACKED_COLOR_BITS * 2)),
        )
    }

    #[must_use]
    pub fn foreground(self) -> Option<CellColor> {
        unpack_color((self.0 & PACKED_COLOR_MASK) as u32)
    }

    #[must_use]
    pub fn background(self) -> Option<CellColor> {
        unpack_color(((self.0 >> PACKED_COLOR_BITS) & PACKED_COLOR_MASK) as u32)
    }

    #[must_use]
    pub fn bold(self) -> bool {
        self.flags() & FLAG_BOLD != 0
    }

    #[must_use]
    pub fn italic(self) -> bool {
        self.flags() & FLAG_ITALIC != 0
    }

    #[must_use]
    pub fn underline(self) -> bool {
        self.flags() & FLAG_UNDERLINE != 0
    }

    #[must_use]
    pub fn inverse(self) -> bool {
        self.flags() & FLAG_INVERSE != 0
    }

    #[must_use]
    pub fn faint(self) -> bool {
        self.flags() & FLAG_FAINT != 0
    }

    fn flags(self) -> u8 {
        (self.0 >> (PACKED_COLOR_BITS * 2)) as u8
    }
}

/// Wire form of [`CellStyle`]: a flat `[fg, bg, flags]` array instead of a
/// field-name map, avoiding per-cell key strings and nested color objects
/// for the common case of a fully (and often uniquely) styled frame.
impl Serialize for CellStyle {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeTuple;

        let mut tuple = serializer.serialize_tuple(3)?;
        tuple.serialize_element(&pack_color(self.foreground()))?;
        tuple.serialize_element(&pack_color(self.background()))?;
        tuple.serialize_element(&self.flags())?;
        tuple.end()
    }
}

impl<'de> Deserialize<'de> for CellStyle {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct CellStyleVisitor;

        impl<'de> serde::de::Visitor<'de> for CellStyleVisitor {
            type Value = CellStyle;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a [fg, bg, flags] array")
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                let foreground: u32 = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(0, &self))?;
                let background: u32 = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(1, &self))?;
                let flags: u8 = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(2, &self))?;
                Ok(CellStyle::new(
                    unpack_color(foreground),
                    unpack_color(background),
                    flags & FLAG_BOLD != 0,
                    flags & FLAG_ITALIC != 0,
                    flags & FLAG_UNDERLINE != 0,
                    flags & FLAG_INVERSE != 0,
                    flags & FLAG_FAINT != 0,
                ))
            }
        }

        deserializer.deserialize_tuple(3, CellStyleVisitor)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Cell {
    pub contents: CompactString,
    pub style: CellStyle,
    pub selected: bool,
    /// Index into [`ScreenSnapshot::hyperlinks`].
    pub hyperlink: Option<u16>,
}

/// Cells dominate terminal frames, so their wire form avoids maps and nested
/// containers. Plain unselected cells are encoded as just their string;
/// styled cells are `[contents, packed_style]`, with a trailing `true` only
/// for selected cells. Hyperlinked cells append `selected, hyperlink_index`.
/// Both 26-bit colors and five flags fit in one `u64`.
impl Serialize for Cell {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if self.style.is_default() && !self.selected && self.hyperlink.is_none() {
            return serializer.serialize_str(&self.contents);
        }
        use serde::ser::SerializeTuple;
        let len = if self.hyperlink.is_some() {
            4
        } else if self.selected {
            3
        } else {
            2
        };
        let mut tuple = serializer.serialize_tuple(len)?;
        tuple.serialize_element(&self.contents)?;
        tuple.serialize_element(&self.style.0)?;
        if self.selected || self.hyperlink.is_some() {
            tuple.serialize_element(&self.selected)?;
        }
        if let Some(hyperlink) = self.hyperlink {
            tuple.serialize_element(&hyperlink)?;
        }
        tuple.end()
    }
}

impl<'de> Deserialize<'de> for Cell {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct CellVisitor;
        impl<'de> serde::de::Visitor<'de> for CellVisitor {
            type Value = Cell;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a cell string or [contents, packed_style, selected] array")
            }
            fn visit_borrowed_str<E: serde::de::Error>(self, value: &'de str) -> Result<Cell, E> {
                Ok(Cell {
                    contents: CompactString::new(value),
                    ..Cell::default()
                })
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Cell, E> {
                Ok(Cell {
                    contents: CompactString::new(value),
                    ..Cell::default()
                })
            }
            fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Cell, E> {
                Ok(Cell {
                    contents: value.into(),
                    ..Cell::default()
                })
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Cell, A::Error> {
                let contents = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(0, &self))?;
                let packed_style: u64 = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(1, &self))?;
                let selected = seq.next_element()?.unwrap_or(false);
                let hyperlink = seq.next_element()?;
                Ok(Cell {
                    contents,
                    style: CellStyle(packed_style),
                    selected,
                    hyperlink,
                })
            }
        }
        deserializer.deserialize_any(CellVisitor)
    }
}

impl CellStyle {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            contents: " ".into(),
            style: CellStyle::default(),
            selected: false,
            hyperlink: None,
        }
    }
}

impl Cell {
    fn bound_contents(&mut self) {
        if self.contents.len() > MAX_CELL_CONTENT_BYTES
            || self.contents.chars().any(char::is_control)
            || self.contents.graphemes(true).count() > 1
        {
            self.contents = CompactString::new(OVERSIZED_CELL_CONTENT_MARKER);
        }
    }

    /// Cheap guard for cells already known to be a single trusted grapheme
    /// cluster with no control characters (built straight from libghostty
    /// state). libghostty never stores control characters as cell content
    /// and `graphemes_utf8` always yields one grapheme cluster per cell, so
    /// only the byte-length cap from [`Self::bound_contents`] still applies:
    /// libghostty can in principle attach an unbounded run of combining
    /// codepoints to a single base character, so length is the one thing
    /// still worth capping here.
    pub(crate) fn cap_length(&mut self) {
        if self.contents.len() > MAX_CELL_CONTENT_BYTES {
            self.contents = CompactString::new(OVERSIZED_CELL_CONTENT_MARKER);
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorShape {
    Bar,
    Underline,
    /// Unknown future shapes and unsupported emulator-specific shapes render
    /// as the universally representable block cursor.
    #[default]
    #[serde(other)]
    Block,
}

impl CursorShape {
    fn is_block(&self) -> bool {
        *self == Self::Block
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Cursor {
    #[serde(rename = "c")]
    pub column: u16,
    #[serde(rename = "r")]
    pub row: u16,
    #[serde(rename = "v")]
    pub visible: bool,
    // Named MessagePack fields are additive: old clients ignore these keys,
    // while their defaults let new clients read snapshots from old daemons.
    #[serde(rename = "s", default, skip_serializing_if = "CursorShape::is_block")]
    pub shape: CursorShape,
    #[serde(rename = "b", default, skip_serializing_if = "std::ops::Not::not")]
    pub blinking: bool,
}

/// Where the snapshot's viewport sits within the terminal's scrollback.
/// `max_offset_from_bottom` of zero means no scrollback exists yet.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ScrollPosition {
    #[serde(rename = "b")]
    pub offset_from_bottom: usize,
    #[serde(rename = "m")]
    pub max_offset_from_bottom: usize,
}

/// A decoded Kitty image, recompressed as PNG for transport to attached
/// clients. Image generations are process-wide libghostty stamps and let a
/// client avoid retransmitting unchanged pixels to its host terminal.
///
/// Pixels are shared, so cloning a screen never copies them. Connections
/// that negotiated separate image frames receive screens whose images have
/// an empty `png`: a reference to pixels delivered earlier in a
/// `kitty_image` message.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct KittyImage {
    #[serde(rename = "i")]
    pub id: u32,
    #[serde(rename = "g")]
    pub generation: u64,
    #[serde(rename = "d", with = "shared_bytes")]
    pub png: Arc<[u8]>,
}

impl KittyImage {
    /// The same image without its pixels, for screens whose images travel
    /// in separate frames.
    #[must_use]
    pub fn reference(&self) -> Self {
        Self {
            id: self.id,
            generation: self.generation,
            png: Arc::from([]),
        }
    }

    #[must_use]
    pub fn is_reference(&self) -> bool {
        self.png.is_empty()
    }
}

/// MessagePack binary for shared byte buffers, matching `serde_bytes`.
mod shared_bytes {
    use std::sync::Arc;

    use serde::{Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &Arc<[u8]>, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(bytes)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Arc<[u8]>, D::Error> {
        let bytes: serde_bytes::ByteBuf = serde::Deserialize::deserialize(deserializer)?;
        Ok(Arc::from(bytes.into_vec()))
    }
}

/// One visible, non-placeholder Kitty placement in terminal-grid space.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct KittyPlacement {
    #[serde(rename = "i")]
    pub image_id: u32,
    #[serde(rename = "p")]
    pub placement_id: u32,
    #[serde(rename = "c")]
    pub column: i32,
    #[serde(rename = "r")]
    pub row: i32,
    #[serde(rename = "w")]
    pub columns: u32,
    #[serde(rename = "h")]
    pub rows: u32,
    #[serde(rename = "x")]
    pub source_x: u32,
    #[serde(rename = "y")]
    pub source_y: u32,
    #[serde(rename = "s")]
    pub source_width: u32,
    #[serde(rename = "t")]
    pub source_height: u32,
    #[serde(rename = "z")]
    pub z: i32,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct KittyGraphics {
    #[serde(rename = "i")]
    pub images: Vec<KittyImage>,
    #[serde(rename = "p")]
    pub placements: Vec<KittyPlacement>,
}

impl KittyGraphics {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.placements.is_empty()
    }

    /// Replace every image with a pixel-free reference.
    pub fn strip_pixels(&mut self) {
        for image in &mut self.images {
            *image = image.reference();
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ScreenSnapshot {
    #[serde(rename = "r")]
    pub revision: u64,
    #[serde(rename = "s")]
    pub size: TerminalSize,
    #[serde(rename = "c")]
    pub cells: Vec<Cell>,
    /// Deduplicated OSC 8 hyperlink URIs referenced by cells.
    #[serde(rename = "h", default, skip_serializing_if = "Vec::is_empty")]
    pub hyperlinks: Vec<CompactString>,
    #[serde(rename = "p")]
    pub cursor: Cursor,
    // Defaulted so snapshots from daemons predating scroll metrics decode.
    #[serde(rename = "v", default)]
    pub scroll: ScrollPosition,
    /// Whether the pane application currently owns mouse button gestures.
    #[serde(rename = "m", default, skip_serializing_if = "std::ops::Not::not")]
    pub mouse_tracking: bool,
    #[serde(rename = "g", default, skip_serializing_if = "KittyGraphics::is_empty")]
    pub graphics: KittyGraphics,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum SnapshotError {
    #[error(transparent)]
    InvalidSize(#[from] TerminalSizeError),
    #[error("snapshot has {actual} cells but its dimensions require {expected}")]
    CellCount { expected: usize, actual: usize },
    #[error("cursor ({column}, {row}) is outside the snapshot")]
    CursorOutOfBounds { column: u16, row: u16 },
}

impl ScreenSnapshot {
    pub fn new(
        revision: u64,
        size: TerminalSize,
        mut cells: Vec<Cell>,
        cursor: Cursor,
    ) -> Result<Self, SnapshotError> {
        Self::check_bounds(size, &cells, cursor)?;
        for cell in &mut cells {
            cell.bound_contents();
        }
        Ok(Self::new_unchecked(revision, size, cells, cursor))
    }

    /// Build a snapshot from cells the caller has already produced in a
    /// trusted, well-formed shape (e.g. straight from libghostty state).
    /// Size and cursor bounds are still checked, and each cell still gets
    /// the cheap byte-length cap (see [`Cell::cap_length`]), but the
    /// expensive control-character scan and grapheme-cluster count that
    /// [`Self::new`] performs for untrusted callers are skipped.
    ///
    /// Only use this for cells that are known by construction to already
    /// be a single grapheme cluster with no control characters.
    pub(crate) fn from_terminal(
        revision: u64,
        size: TerminalSize,
        mut cells: Vec<Cell>,
        hyperlinks: Vec<CompactString>,
        cursor: Cursor,
    ) -> Result<Self, SnapshotError> {
        Self::check_bounds(size, &cells, cursor)?;
        for cell in &mut cells {
            cell.cap_length();
        }
        let mut snapshot = Self::new_unchecked(revision, size, cells, cursor);
        snapshot.hyperlinks = hyperlinks;
        Ok(snapshot)
    }

    fn check_bounds(
        size: TerminalSize,
        cells: &[Cell],
        cursor: Cursor,
    ) -> Result<(), SnapshotError> {
        let expected = size.cell_count()?;
        if cells.len() != expected {
            return Err(SnapshotError::CellCount {
                expected,
                actual: cells.len(),
            });
        }
        if cursor.column >= size.columns || cursor.row >= size.rows {
            return Err(SnapshotError::CursorOutOfBounds {
                column: cursor.column,
                row: cursor.row,
            });
        }
        Ok(())
    }

    fn new_unchecked(revision: u64, size: TerminalSize, cells: Vec<Cell>, cursor: Cursor) -> Self {
        Self {
            revision,
            size,
            cells,
            hyperlinks: Vec::new(),
            cursor,
            scroll: ScrollPosition::default(),
            mouse_tracking: false,
            graphics: KittyGraphics::default(),
        }
    }

    /// Atomically applies a delta produced from this exact snapshot.
    ///
    /// All of `delta` is checked before this snapshot is changed. A rejected
    /// delta therefore leaves the receiver usable as the base for a later
    /// refresh or a correctly sequenced delta.
    pub fn apply_delta(&mut self, delta: ScreenDelta) -> Result<(), ScreenDeltaError> {
        if delta.base_revision != self.revision {
            return Err(ScreenDeltaError::BaseRevisionMismatch {
                expected: self.revision,
                actual: delta.base_revision,
            });
        }
        if delta.size != self.size {
            return Err(ScreenDeltaError::SizeMismatch {
                expected: self.size,
                actual: delta.size,
            });
        }
        if delta.revision <= self.revision {
            return Err(ScreenDeltaError::RevisionNotNewer {
                current: self.revision,
                actual: delta.revision,
            });
        }

        let columns = usize::from(self.size.columns);
        let rows = usize::from(self.size.rows);
        let mut seen_rows = vec![false; rows];
        for row in &delta.rows {
            let index = usize::from(row.index);
            if index >= rows {
                return Err(ScreenDeltaError::RowOutOfBounds {
                    row: row.index,
                    rows: self.size.rows,
                });
            }
            if std::mem::replace(&mut seen_rows[index], true) {
                return Err(ScreenDeltaError::DuplicateRow { row: row.index });
            }
            if row.cells.len() != columns {
                return Err(ScreenDeltaError::RowWidth {
                    row: row.index,
                    expected: columns,
                    actual: row.cells.len(),
                });
            }
        }
        if delta.cursor.column >= self.size.columns || delta.cursor.row >= self.size.rows {
            return Err(ScreenDeltaError::CursorOutOfBounds {
                column: delta.cursor.column,
                row: delta.cursor.row,
            });
        }
        if let Some(shift) = delta.shift
            && !shift.fits(self.size.rows)
        {
            return Err(ScreenDeltaError::ShiftOutOfBounds {
                shift,
                rows: self.size.rows,
            });
        }

        if let Some(shift) = delta.shift {
            let region = &mut self.cells
                [usize::from(shift.top) * columns..usize::from(shift.bottom) * columns];
            region.rotate_left(shift.rotation() * columns);
        }
        for row in delta.rows {
            let start = usize::from(row.index) * columns;
            self.cells[start..start + columns].clone_from_slice(&row.cells);
        }
        self.revision = delta.revision;
        self.hyperlinks = delta.hyperlinks;
        self.cursor = delta.cursor;
        self.scroll = delta.scroll;
        self.mouse_tracking = delta.mouse_tracking;
        if let Some(graphics) = delta.graphics {
            self.graphics = graphics;
        }
        Ok(())
    }

    fn row(&self, index: usize) -> &[Cell] {
        let columns = usize::from(self.size.columns);
        &self.cells[index * columns..(index + 1) * columns]
    }

    /// The delta that turns `previous` into this screen, or `None` when a
    /// full screen is the better message: the sizes differ, or more than
    /// [`DELTA_ROW_FALLBACK_THRESHOLD`] of the rows still need sending.
    ///
    /// With `shifts`, a delta may also move retained rows ([`RowShift`]) so
    /// scrolling output carries only the rows it exposed. Peers that predate
    /// shifts must be sent deltas produced without them.
    #[must_use]
    pub fn delta_from(&self, previous: &ScreenSnapshot, shifts: bool) -> Option<ScreenDelta> {
        if previous.size != self.size {
            return None;
        }
        let (shift, rows) = changed_rows(previous, self, shifts)?;
        Some(ScreenDelta {
            revision: self.revision,
            base_revision: previous.revision,
            size: self.size,
            shift,
            rows,
            hyperlinks: self.hyperlinks.clone(),
            cursor: self.cursor,
            scroll: self.scroll,
            mouse_tracking: self.mouse_tracking,
            graphics: (previous.graphics != self.graphics).then(|| self.graphics.clone()),
        })
    }
}

/// Rows changed exceeding this fraction of the grid fall back to a full
/// snapshot: past this point the delta's per-row framing overhead outweighs
/// what it saves over just resending everything.
pub const DELTA_ROW_FALLBACK_THRESHOLD: f64 = 0.6;

/// Fewest changed rows worth looking for a shift behind: typing and cursor
/// movement change one or two rows, and a shift cannot save anything there.
const SHIFT_MIN_CHANGED_ROWS: usize = 3;
/// Rows of the new screen whose old positions propose candidate shifts, and
/// the nearest matches kept for each anchor and direction. Repeated rows
/// (`yes`, blank lines) match many offsets; nearer ones are tried first.
const SHIFT_ANCHORS: usize = 3;
const SHIFT_CANDIDATES_PER_ANCHOR: usize = 4;

/// Rows of `next` to send, with an optional shift of `previous`'s rows applied
/// first, or `None` if that's more than [`DELTA_ROW_FALLBACK_THRESHOLD`] of all
/// rows. Callers must already know `previous.size == next.size`.
fn changed_rows(
    previous: &ScreenSnapshot,
    next: &ScreenSnapshot,
    shifts: bool,
) -> Option<(Option<RowShift>, Vec<DeltaRow>)> {
    let rows = usize::from(next.size.rows);
    let changed: Vec<usize> = (0..rows)
        .filter(|&index| previous.row(index) != next.row(index))
        .collect();
    let shifted = (shifts && changed.len() >= SHIFT_MIN_CHANGED_ROWS)
        .then(|| find_shift(previous, next, &changed))
        .flatten()
        .filter(|(_, remaining)| remaining.len() < changed.len());
    let (shift, send) = match shifted {
        Some((shift, remaining)) => (Some(shift), remaining),
        None => (None, changed),
    };
    if send.len() as f64 > rows as f64 * DELTA_ROW_FALLBACK_THRESHOLD {
        return None;
    }
    let rows = send
        .into_iter()
        .map(|index| DeltaRow {
            index: index as u16,
            cells: next.row(index).to_vec(),
        })
        .collect();
    Some((shift, rows))
}

/// The shift of `previous`'s rows that leaves the fewest rows of `next` to
/// send, and those rows. `changed` lists the rows that differ in place,
/// ascending and nonempty; only their span can move.
///
/// A few anchor rows of `next` propose candidate shifts by exact comparison
/// against `previous`, which rejects most rows within their first cells, so
/// output that shares no rows with the last screen costs a few hundred short
/// comparisons. Only then are rows hashed to rank the candidates. The rows
/// returned come from exact comparison, so a hash collision costs bytes, never
/// correctness.
fn find_shift(
    previous: &ScreenSnapshot,
    next: &ScreenSnapshot,
    changed: &[usize],
) -> Option<(RowShift, Vec<usize>)> {
    let top = changed[0];
    let bottom = changed[changed.len() - 1] + 1;
    let span = bottom - top;

    // Content that scrolled up by `by` sits at `anchor` now and at
    // `anchor + by` before; scrolling down puts it above.
    let mut candidates: Vec<isize> = Vec::new();
    for anchor in
        (0..SHIFT_ANCHORS).map(|n| changed[n * (changed.len() - 1) / (SHIFT_ANCHORS - 1)] - top)
    {
        let row = next.row(top + anchor);
        let matches = |source: &usize| previous.row(top + source) == row;
        let up = (anchor + 1..span)
            .filter(matches)
            .take(SHIFT_CANDIDATES_PER_ANCHOR)
            .map(|source| (source - anchor) as isize);
        let down = (0..anchor)
            .rev()
            .filter(matches)
            .take(SHIFT_CANDIDATES_PER_ANCHOR)
            .map(|source| -((anchor - source) as isize));
        for by in up.chain(down) {
            if !candidates.contains(&by) && i16::try_from(by).is_ok() {
                candidates.push(by);
            }
        }
    }
    if candidates.is_empty() {
        return None;
    }

    let old: &[u64] = &(top..bottom)
        .map(|index| row_hash(previous.row(index)))
        .collect::<Vec<_>>();
    let new: &[u64] = &(top..bottom)
        .map(|index| row_hash(next.row(index)))
        .collect::<Vec<_>>();
    let retained = move |by: isize| {
        (0..span).filter(move |&index| {
            let source = index as isize + by;
            (0..span as isize).contains(&source) && new[index] == old[source as usize]
        })
    };
    let by = candidates
        .into_iter()
        .map(|by| (retained(by).count(), by))
        .filter(|&(count, _)| count > 0)
        .max_by_key(|&(count, by)| (count, std::cmp::Reverse(by.unsigned_abs()), by > 0))?
        .1;

    // Move only the region that holds retained rows and the rows they
    // expose, so unchanged rows around a scroll region stay put.
    let first = retained(by).next()?;
    let last = retained(by).next_back()?;
    let (start, end) = if by > 0 {
        (first, last + by as usize + 1)
    } else {
        (first - by.unsigned_abs(), last + 1)
    };
    let shift = RowShift {
        top: (top + start) as u16,
        bottom: (top + end) as u16,
        by: by as i16,
    };
    let remaining = (top..bottom)
        .filter(|&index| {
            let source = shift.source(index);
            !(new[index - top] == old[source - top] && next.row(index) == previous.row(source))
        })
        .collect();
    Some((shift, remaining))
}

/// A fast, non-cryptographic fingerprint of a row's cells for ranking
/// shifts; equal rows always hash equal. Each cell folds into the running
/// hash with a single multiply, which bounds the cost on large screens.
fn row_hash(cells: &[Cell]) -> u64 {
    const MULTIPLIER: u64 = 0xf135_7aea_2e62_a9c5;
    const SPREAD: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut hash = 0_u64;
    for cell in cells {
        let bytes = cell.contents.as_bytes();
        let mut chunks = bytes.chunks(8);
        let word = |chunk: &[u8]| {
            let mut word = [0_u8; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            u64::from_le_bytes(word)
        };
        let mut contents = chunks.next().map_or(0, word);
        for chunk in chunks {
            contents = (contents ^ word(chunk))
                .wrapping_mul(SPREAD)
                .rotate_left(31);
        }
        let attributes = cell.style.0
            ^ ((bytes.len() as u64) << 57)
            ^ (u64::from(cell.selected) << 63)
            ^ u64::from(cell.hyperlink.map_or(0, |index| u32::from(index) + 1)).rotate_left(29);
        hash = (hash ^ contents.wrapping_mul(SPREAD) ^ attributes)
            .wrapping_mul(MULTIPLIER)
            .rotate_left(26);
    }
    hash
}

/// Rows of a [`ScreenDelta`]'s base that moved: rows `top..bottom` rotate up
/// by `by` rows (down when negative) before the delta's replacement rows
/// apply. The rows a shift exposes hold whatever rotated into them until
/// those replacements arrive, so scrolling output carries only new rows.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RowShift {
    #[serde(rename = "t")]
    pub top: u16,
    /// Exclusive.
    #[serde(rename = "e")]
    pub bottom: u16,
    #[serde(rename = "n")]
    pub by: i16,
}

impl RowShift {
    /// A nonempty region within `rows` that the shift actually moves.
    fn fits(self, rows: u16) -> bool {
        self.top < self.bottom
            && self.bottom <= rows
            && self.by != 0
            && self.by.unsigned_abs() < self.bottom - self.top
    }

    /// Rows `top..bottom` rotate left by this many rows. Requires [`Self::fits`].
    fn rotation(self) -> usize {
        let span = i32::from(self.bottom - self.top);
        i32::from(self.by).rem_euclid(span) as usize
    }

    /// Row of the base grid that lands at `row`. Requires [`Self::fits`].
    fn source(self, row: usize) -> usize {
        let top = usize::from(self.top);
        let bottom = usize::from(self.bottom);
        if !(top..bottom).contains(&row) {
            return row;
        }
        top + (row - top + self.rotation()) % (bottom - top)
    }
}

/// One changed row of a [`ScreenDelta`]: its index within the grid plus a
/// full replacement row of cells.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DeltaRow {
    #[serde(rename = "i")]
    pub index: u16,
    #[serde(rename = "c")]
    pub cells: Vec<Cell>,
}

/// Rows that changed since `base_revision`, in place of a full
/// [`ScreenSnapshot`]. Only valid against a grid the receiver already has at
/// exactly `base_revision` and `size`; anything else and the receiver must
/// fall back to requesting a full snapshot.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ScreenDelta {
    #[serde(rename = "r")]
    pub revision: u64,
    #[serde(rename = "b")]
    pub base_revision: u64,
    #[serde(rename = "s")]
    pub size: TerminalSize,
    /// Moves retained rows before `rows` apply. Only sent to peers that
    /// understand it: local clients and remote `scroll-deltas.v1`.
    #[serde(rename = "x", default, skip_serializing_if = "Option::is_none")]
    pub shift: Option<RowShift>,
    #[serde(rename = "d")]
    pub rows: Vec<DeltaRow>,
    #[serde(rename = "h", default, skip_serializing_if = "Vec::is_empty")]
    pub hyperlinks: Vec<CompactString>,
    #[serde(rename = "p")]
    pub cursor: Cursor,
    #[serde(rename = "v")]
    pub scroll: ScrollPosition,
    #[serde(rename = "m", default, skip_serializing_if = "std::ops::Not::not")]
    pub mouse_tracking: bool,
    /// Complete graphics state when it changed; omitted for ordinary text,
    /// cursor, and scroll updates so image bytes are not resent every frame.
    #[serde(rename = "g", default, skip_serializing_if = "Option::is_none")]
    pub graphics: Option<KittyGraphics>,
}

/// Why a [`ScreenDelta`] could not be applied to a [`ScreenSnapshot`].
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ScreenDeltaError {
    #[error("delta base revision {actual} does not match snapshot revision {expected}")]
    BaseRevisionMismatch { expected: u64, actual: u64 },
    #[error("delta size {actual:?} does not match snapshot size {expected:?}")]
    SizeMismatch {
        expected: TerminalSize,
        actual: TerminalSize,
    },
    #[error("delta revision {actual} is not newer than snapshot revision {current}")]
    RevisionNotNewer { current: u64, actual: u64 },
    #[error("delta row {row} is outside the {rows}-row snapshot")]
    RowOutOfBounds { row: u16, rows: u16 },
    #[error("delta contains row {row} more than once")]
    DuplicateRow { row: u16 },
    #[error("delta row {row} has {actual} cells; expected {expected}")]
    RowWidth {
        row: u16,
        expected: usize,
        actual: usize,
    },
    #[error("delta cursor ({column}, {row}) is outside the snapshot")]
    CursorOutOfBounds { column: u16, row: u16 },
    #[error("delta shift {shift:?} does not fit the {rows}-row snapshot")]
    ShiftOutOfBounds { shift: RowShift, rows: u16 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn program_status_derives_summary_attention_and_inherited_app() {
        let mut status = ProgramStatus::default();
        status.apply_updates(
            vec![
                StatusUpdate::Report(status_report("", StatusState::Working).with_app("cargo")),
                StatusUpdate::Report(status_report("tests/unit", StatusState::Blocked)),
            ],
            StatusSource::Osc,
            4,
            12,
        );

        assert_eq!(status.root().unwrap().state, StatusState::Working);
        assert_eq!(status.summary_state(), StatusState::Blocked);
        assert_eq!(status.app(status.summary_record().unwrap()), Some("cargo"));
        assert_eq!(status.attention().unwrap().id, "tests/unit");
        assert!(status.has_unread_attention());
        assert_eq!(status.working_revision, 4);

        let blocked_attention = status.attention_revision;
        status.apply_updates(
            vec![StatusUpdate::Report(StatusReport {
                progress: Some(50),
                ..status_report("tests/unit", StatusState::Blocked)
            })],
            StatusSource::Osc,
            5,
            13,
        );
        assert_eq!(status.attention_revision, blocked_attention);
        assert_eq!(
            status.attention().unwrap().attention_revision,
            blocked_attention
        );

        status.apply_updates(
            vec![StatusUpdate::Report(StatusReport {
                progress: Some(60),
                title: Some("A different request".into()),
                ..status_report("tests/unit", StatusState::Blocked)
            })],
            StatusSource::Osc,
            6,
            14,
        );
        assert_eq!(status.attention_revision, 6);
        assert_eq!(status.attention().unwrap().attention_revision, 6);

        status.apply_updates(
            vec![StatusUpdate::Report(status_report(
                "tests/unit",
                StatusState::Done,
            ))],
            StatusSource::Osc,
            7,
            15,
        );
        assert_eq!(status.attention_revision, 7);
        assert_eq!(status.summary_state(), StatusState::Working);
        assert!(
            status.attention().is_none(),
            "a done sub-record waits for the working root"
        );
        assert!(!status.has_unread_attention());

        status.apply_updates(
            vec![StatusUpdate::Report(status_report(
                "failure",
                StatusState::Error,
            ))],
            StatusSource::Osc,
            8,
            16,
        );
        assert_eq!(status.summary_state(), StatusState::Error);

        status.apply_updates(
            vec![StatusUpdate::Report(
                status_report("", StatusState::Working).with_app("next"),
            )],
            StatusSource::Osc,
            9,
            17,
        );
        assert_eq!(status.app(status.records.last().unwrap()), Some("next"));
        let child = status
            .records
            .iter()
            .find(|record| record.id == "tests/unit")
            .unwrap();
        assert_eq!(status.app(child), Some("next"));
        assert_eq!(status.label(), Some("next"));
        assert!(status.is_reporting());
    }

    #[test]
    fn status_progress_prefers_root_and_avoids_ambiguous_workers() {
        let mut status = ProgramStatus::default();
        let mut root = status_report("", StatusState::Working);
        root.progress = Some(10);
        let mut child = status_report("child", StatusState::Working);
        child.progress = Some(90);
        let mut other_child = status_report("other", StatusState::Working);
        other_child.progress = Some(70);
        status.apply_updates(
            vec![
                StatusUpdate::Report(child),
                StatusUpdate::Report(other_child),
                StatusUpdate::Report(root),
            ],
            StatusSource::Osc,
            1,
            1,
        );
        assert_eq!(status.progress(), Some(10));
        assert!(
            status.records[0].id.is_empty(),
            "root is always exposed first"
        );

        status.apply_updates(
            vec![StatusUpdate::Clear { id: None }],
            StatusSource::Osc,
            2,
            2,
        );
        let mut first = status_report("first", StatusState::Working);
        first.progress = Some(30);
        let mut second = status_report("second", StatusState::Working);
        second.progress = Some(60);
        status.apply_updates(
            vec![StatusUpdate::Report(first), StatusUpdate::Report(second)],
            StatusSource::Osc,
            3,
            3,
        );
        assert_eq!(status.progress(), None);

        status.apply_updates(
            vec![
                StatusUpdate::Clear { id: None },
                StatusUpdate::Report(StatusReport {
                    progress: Some(40),
                    ..status_report("blocked", StatusState::Blocked)
                }),
            ],
            StatusSource::Osc,
            4,
            4,
        );
        assert_eq!(
            status.progress(),
            Some(40),
            "a sole non-root blocker can report determinate progress"
        );

        status.apply_updates(
            vec![StatusUpdate::Report(StatusReport {
                progress: Some(30),
                ..status_report("worker", StatusState::Working)
            })],
            StatusSource::Osc,
            5,
            5,
        );
        assert_eq!(
            status.progress(),
            None,
            "working and blocked records make non-root progress ambiguous"
        );

        status.apply_updates(
            vec![
                StatusUpdate::Clear { id: None },
                StatusUpdate::Report(StatusReport {
                    progress: Some(45),
                    ..status_report("", StatusState::Blocked)
                }),
            ],
            StatusSource::Osc,
            6,
            6,
        );
        assert_eq!(status.progress(), Some(45));

        status.apply_updates(
            vec![
                StatusUpdate::Clear { id: None },
                StatusUpdate::Report(status_report("old-error", StatusState::Error)),
                StatusUpdate::Report(StatusReport {
                    progress: Some(40),
                    ..status_report("active", StatusState::Working)
                }),
            ],
            StatusSource::Osc,
            7,
            7,
        );
        assert_eq!(status.summary_state(), StatusState::Error);
        assert_eq!(status.progress(), Some(40));

        let mut detected = ProgramStatus::default();
        let mut detected_report = status_report("", StatusState::Working);
        detected_report.title = Some("not program identity".into());
        detected_report.message = Some("also not identity".into());
        detected.apply_updates(
            vec![StatusUpdate::Report(detected_report)],
            StatusSource::Detection {
                agent: "codex".into(),
                rule: "working_indicator".into(),
            },
            1,
            1,
        );
        assert!(!detected.is_reporting());
        assert_eq!(detected.label(), Some("codex"));

        let mut legacy = ProgramStatus::default();
        legacy.apply_updates(
            vec![StatusUpdate::Report(status_report(
                "",
                StatusState::Working,
            ))],
            StatusSource::legacy_progress(),
            1,
            1,
        );
        assert_eq!(legacy.label(), None, "OSC 9;4 names no program");
    }

    #[test]
    fn status_summary_uses_protocol_priority_then_update_time_and_label_uses_identity() {
        let mut status = ProgramStatus::default();
        status.apply_updates(
            vec![
                StatusUpdate::Report(status_report("idle", StatusState::Idle)),
                StatusUpdate::Report(status_report("done", StatusState::Done)),
                StatusUpdate::Report(status_report("working", StatusState::Working)),
                StatusUpdate::Report(status_report("error", StatusState::Error)),
                StatusUpdate::Report(status_report("blocked", StatusState::Blocked)),
            ],
            StatusSource::Report {
                source: Some("manual".into()),
                agent_session_id: None,
            },
            1,
            10,
        );
        assert_eq!(status.summary_record().unwrap().id, "blocked");

        status.apply_updates(
            vec![StatusUpdate::Clear { id: None }],
            StatusSource::Osc,
            2,
            20,
        );
        status.apply_updates(
            vec![StatusUpdate::Report(StatusReport {
                title: Some("not identity".into()),
                ..status_report("newer-time", StatusState::Working)
            })],
            StatusSource::Report {
                source: Some("manual".into()),
                agent_session_id: None,
            },
            3,
            100,
        );
        status.apply_updates(
            vec![StatusUpdate::Report(status_report(
                "newer-revision",
                StatusState::Working,
            ))],
            StatusSource::Report {
                source: Some("manual".into()),
                agent_session_id: None,
            },
            4,
            50,
        );
        assert_eq!(status.summary_record().unwrap().id, "newer-revision");
        assert_eq!(status.label(), Some("manual"));
    }

    #[test]
    fn status_records_are_whole_replacements_and_subtree_clears_have_boundaries() {
        let mut status = ProgramStatus::default();
        for index in 0..MAX_STATUS_RECORDS {
            status.apply_updates(
                vec![StatusUpdate::Report(
                    status_report(&format!("r{index}"), StatusState::Idle).with_app("app"),
                )],
                StatusSource::Osc,
                index as u64 + 1,
                index as u64,
            );
        }
        status.apply_updates(
            vec![
                StatusUpdate::Report(status_report("r0", StatusState::Idle)),
                StatusUpdate::Report(status_report("overflow", StatusState::Idle)),
            ],
            StatusSource::Osc,
            300,
            300,
        );
        assert_eq!(status.records.len(), MAX_STATUS_RECORDS);
        assert_eq!(
            status
                .records
                .iter()
                .find(|record| record.id == "r0")
                .unwrap()
                .app,
            None,
            "a report replaces every optional field"
        );
        assert!(!status.records.iter().any(|record| record.id == "r1"));

        status.apply_updates(
            vec![StatusUpdate::Clear {
                id: Some("r2".into()),
            }],
            StatusSource::Osc,
            301,
            301,
        );
        assert!(status.records.iter().any(|record| record.id == "r20"));
    }

    #[test]
    fn status_validation_enforces_protocol_bounds_and_controls() {
        assert!(
            status_report(&"x".repeat(MAX_STATUS_ID_SEGMENT_BYTES), StatusState::Idle)
                .validate()
                .is_ok()
        );
        assert_eq!(
            status_report(
                &"x".repeat(MAX_STATUS_ID_SEGMENT_BYTES + 1),
                StatusState::Idle
            )
            .validate(),
            Err("invalid status id")
        );
        assert_eq!(
            StatusReport {
                message: Some("spoof\u{009b}".into()),
                ..status_report("", StatusState::Done)
            }
            .validate(),
            Err("invalid status message")
        );
        assert_eq!(
            ProgramStatus::validate_source(&StatusSource::Detection {
                agent: "codex".into(),
                rule: "bad\nrule".into(),
            }),
            Err("invalid status source metadata")
        );
    }

    fn status_report(id: &str, state: StatusState) -> StatusReport {
        StatusReport {
            id: id.into(),
            state,
            kind: None,
            progress: None,
            app: None,
            title: None,
            message: None,
        }
    }

    trait StatusReportTestExt {
        fn with_app(self, app: &str) -> Self;
    }

    impl StatusReportTestExt for StatusReport {
        fn with_app(mut self, app: &str) -> Self {
            self.app = Some(app.into());
            self
        }
    }

    #[test]
    fn ids_are_compact_unambiguous_and_serde_round_trip_as_uuid() {
        let first = TerminalId::new();
        let second = TerminalId::new();
        assert_ne!(first, second);

        let json = serde_json::to_string(&first).unwrap();
        assert_eq!(serde_json::from_str::<TerminalId>(&json).unwrap(), first);
        assert_eq!(first.to_string().len(), COMPACT_ID_LEN);
        assert_eq!(first.to_string().parse::<TerminalId>().unwrap(), first);
        assert_eq!(
            first.uuid().to_string().parse::<TerminalId>().unwrap(),
            first
        );
        assert_eq!(json, format!("\"{}\"", first.uuid()));
        for malformed in ["short", &format!("{}A", first), &format!("{}=", first)] {
            assert!(malformed.parse::<TerminalId>().is_err());
        }
    }

    #[test]
    fn snapshot_accepts_a_row_major_grid() {
        let snapshot = ScreenSnapshot::new(
            7,
            TerminalSize {
                columns: 2,
                rows: 2,
            },
            vec![Cell::default(); 4],
            Cursor {
                column: 1,
                row: 1,
                visible: true,
                shape: Default::default(),
                blinking: false,
            },
        )
        .unwrap();

        assert_eq!(snapshot.revision, 7);
        assert_eq!(snapshot.cells.len(), 4);
    }

    #[test]
    fn snapshot_preserves_normal_graphemes_and_marks_overlong_cell_content() {
        let normal = ["a", "é", "雪", "😀", "👍🏽", "👨‍👩‍👧‍👦"];
        let mut cells = normal
            .iter()
            .map(|contents| Cell {
                contents: (*contents).into(),
                ..Cell::default()
            })
            .collect::<Vec<_>>();
        cells.push(Cell {
            contents: format!("a{}", "\u{301}".repeat(MAX_CELL_CONTENT_BYTES)).into(),
            ..Cell::default()
        });
        let snapshot = ScreenSnapshot::new(
            1,
            TerminalSize {
                columns: cells.len() as u16,
                rows: 1,
            },
            cells,
            Cursor {
                column: 0,
                row: 0,
                visible: true,
                shape: Default::default(),
                blinking: false,
            },
        )
        .unwrap();

        assert_eq!(
            snapshot.cells[..normal.len()]
                .iter()
                .map(|cell| cell.contents.as_str())
                .collect::<Vec<_>>(),
            normal
        );
        assert_eq!(
            snapshot.cells.last().unwrap().contents,
            OVERSIZED_CELL_CONTENT_MARKER
        );
        assert!(
            snapshot
                .cells
                .iter()
                .all(|cell| cell.contents.len() <= MAX_CELL_CONTENT_BYTES)
        );
    }

    #[test]
    fn snapshot_rejects_invalid_dimensions_cells_and_cursor() {
        let cursor = Cursor {
            column: 0,
            row: 0,
            visible: true,
            shape: Default::default(),
            blinking: false,
        };
        assert_eq!(
            ScreenSnapshot::new(
                0,
                TerminalSize {
                    columns: 0,
                    rows: 1
                },
                vec![],
                cursor
            ),
            Err(SnapshotError::InvalidSize(TerminalSizeError::Empty))
        );
        assert!(matches!(
            ScreenSnapshot::new(
                0,
                TerminalSize {
                    columns: 2,
                    rows: 1
                },
                vec![Cell::default()],
                cursor
            ),
            Err(SnapshotError::CellCount { .. })
        ));
        assert!(matches!(
            ScreenSnapshot::new(
                0,
                TerminalSize {
                    columns: 1,
                    rows: 1
                },
                vec![Cell::default()],
                Cursor {
                    column: 1,
                    ..cursor
                }
            ),
            Err(SnapshotError::CursorOutOfBounds { .. })
        ));
    }

    #[test]
    fn snapshot_rejects_excessive_dimensions_before_cell_validation() {
        let size = TerminalSize {
            columns: u16::MAX,
            rows: u16::MAX,
        };
        assert!(matches!(
            ScreenSnapshot::new(
                0,
                size,
                Vec::new(),
                Cursor {
                    column: 0,
                    row: 0,
                    visible: false,
                    shape: Default::default(),
                    blinking: false,
                },
            ),
            Err(SnapshotError::InvalidSize(TerminalSizeError::TooLarge {
                maximum: MAX_VISIBLE_CELLS,
                ..
            }))
        ));
    }

    fn delta_snapshot() -> ScreenSnapshot {
        ScreenSnapshot::new(
            10,
            TerminalSize {
                columns: 2,
                rows: 2,
            },
            vec![
                Cell {
                    contents: "a".into(),
                    hyperlink: Some(0),
                    ..Cell::default()
                },
                Cell {
                    contents: "b".into(),
                    ..Cell::default()
                },
                Cell {
                    contents: "c".into(),
                    ..Cell::default()
                },
                Cell {
                    contents: "d".into(),
                    ..Cell::default()
                },
            ],
            Cursor {
                column: 0,
                row: 0,
                visible: true,
                shape: CursorShape::Block,
                blinking: false,
            },
        )
        .unwrap()
    }

    fn valid_delta(revision: u64) -> ScreenDelta {
        ScreenDelta {
            revision,
            base_revision: 10,
            size: TerminalSize {
                columns: 2,
                rows: 2,
            },
            shift: None,
            rows: vec![DeltaRow {
                index: 1,
                cells: vec![
                    Cell {
                        contents: "x".into(),
                        hyperlink: Some(0),
                        ..Cell::default()
                    },
                    Cell {
                        contents: "y".into(),
                        ..Cell::default()
                    },
                ],
            }],
            hyperlinks: vec!["https://example.test/new".into()],
            cursor: Cursor {
                column: 1,
                row: 1,
                visible: true,
                shape: CursorShape::Underline,
                blinking: true,
            },
            scroll: ScrollPosition {
                offset_from_bottom: 3,
                max_offset_from_bottom: 8,
            },
            mouse_tracking: true,
            graphics: None,
        }
    }

    #[test]
    fn delta_produces_the_same_complete_state_as_a_full_snapshot() {
        let mut from_delta = delta_snapshot();
        let mut delta = valid_delta(11);
        delta.graphics = Some(KittyGraphics {
            images: vec![KittyImage {
                id: 7,
                generation: 4,
                png: vec![1, 2, 3].into(),
            }],
            placements: vec![KittyPlacement {
                image_id: 7,
                placement_id: 2,
                column: 0,
                row: 1,
                columns: 1,
                rows: 1,
                source_x: 0,
                source_y: 0,
                source_width: 1,
                source_height: 1,
                z: 0,
            }],
        });
        from_delta.apply_delta(delta).unwrap();

        let mut full = ScreenSnapshot::new(
            11,
            from_delta.size,
            vec![
                Cell {
                    contents: "a".into(),
                    hyperlink: Some(0),
                    ..Cell::default()
                },
                Cell {
                    contents: "b".into(),
                    ..Cell::default()
                },
                Cell {
                    contents: "x".into(),
                    hyperlink: Some(0),
                    ..Cell::default()
                },
                Cell {
                    contents: "y".into(),
                    ..Cell::default()
                },
            ],
            Cursor {
                column: 1,
                row: 1,
                visible: true,
                shape: CursorShape::Underline,
                blinking: true,
            },
        )
        .unwrap();
        full.hyperlinks = vec!["https://example.test/new".into()];
        full.scroll = ScrollPosition {
            offset_from_bottom: 3,
            max_offset_from_bottom: 8,
        };
        full.mouse_tracking = true;
        full.graphics = from_delta.graphics.clone();

        assert_eq!(from_delta, full);
    }

    #[test]
    fn delta_graphics_retain_change_and_clear() {
        let mut snapshot = delta_snapshot();
        let retained = KittyGraphics {
            images: vec![KittyImage {
                id: 1,
                generation: 1,
                png: vec![1].into(),
            }],
            placements: Vec::new(),
        };
        snapshot.graphics = retained.clone();

        snapshot.apply_delta(valid_delta(11)).unwrap();
        assert_eq!(snapshot.graphics, retained);

        let changed = KittyGraphics {
            images: vec![KittyImage {
                id: 2,
                generation: 2,
                png: vec![2].into(),
            }],
            placements: Vec::new(),
        };
        let mut change = valid_delta(12);
        change.base_revision = 11;
        change.graphics = Some(changed.clone());
        snapshot.apply_delta(change).unwrap();
        assert_eq!(snapshot.graphics, changed);

        let mut clear = valid_delta(13);
        clear.base_revision = 12;
        clear.graphics = Some(KittyGraphics::default());
        snapshot.apply_delta(clear).unwrap();
        assert_eq!(snapshot.graphics, KittyGraphics::default());
    }

    #[test]
    fn invalid_delta_is_rejected_without_changing_the_snapshot() {
        let malformed = [
            {
                let mut delta = valid_delta(11);
                delta.base_revision = 9;
                delta
            },
            {
                let mut delta = valid_delta(11);
                delta.size.columns = 3;
                delta
            },
            valid_delta(10),
            {
                let mut delta = valid_delta(11);
                delta.rows[0].index = 2;
                delta
            },
            {
                let mut delta = valid_delta(11);
                delta.rows.push(delta.rows[0].clone());
                delta
            },
            {
                let mut delta = valid_delta(11);
                delta.rows[0].cells.pop();
                delta
            },
            {
                let mut delta = valid_delta(11);
                delta.cursor.column = 2;
                delta
            },
        ];

        for delta in malformed {
            let mut snapshot = delta_snapshot();
            let before = snapshot.clone();
            assert!(snapshot.apply_delta(delta).is_err());
            assert_eq!(snapshot, before);
        }
    }

    /// A 2-column screen whose rows read `lines`, one label per row.
    fn rows_snapshot(revision: u64, lines: &[&str]) -> ScreenSnapshot {
        let cells = lines
            .iter()
            .flat_map(|line| {
                let mut chars = line.chars();
                [chars.next(), chars.next()].map(|ch| Cell {
                    contents: ch.unwrap_or(' ').to_string().into(),
                    ..Cell::default()
                })
            })
            .collect();
        ScreenSnapshot::new(
            revision,
            TerminalSize {
                columns: 2,
                rows: lines.len() as u16,
            },
            cells,
            Cursor {
                column: 0,
                row: 0,
                visible: true,
                shape: CursorShape::Block,
                blinking: false,
            },
        )
        .unwrap()
    }

    fn row_labels(screen: &ScreenSnapshot) -> Vec<String> {
        screen
            .cells
            .chunks(usize::from(screen.size.columns))
            .map(|row| row.iter().map(|cell| cell.contents.as_str()).collect())
            .collect()
    }

    #[test]
    fn scrolled_screens_shift_retained_rows_and_carry_only_exposed_ones() {
        let base = rows_snapshot(1, &["a ", "b ", "c ", "d ", "e ", "f "]);
        let cases: [(&[&str], RowShift, &[u16]); 4] = [
            // Output scrolled the whole screen up by two lines.
            (
                &["c ", "d ", "e ", "f ", "g ", "h "],
                RowShift {
                    top: 0,
                    bottom: 6,
                    by: 2,
                },
                &[4, 5],
            ),
            // A pager's reverse index scrolled down above a status line.
            (
                &["y ", "z ", "a ", "b ", "c ", "f "],
                RowShift {
                    top: 0,
                    bottom: 5,
                    by: -2,
                },
                &[0, 1],
            ),
            // A scroll region in the middle, plus an edit to the first row.
            (
                &["A ", "c ", "d ", "e ", "q ", "f "],
                RowShift {
                    top: 1,
                    bottom: 5,
                    by: 1,
                },
                &[0, 4],
            ),
            // Lines deleted below the cursor pull the rest up.
            (
                &["a ", "b ", "e ", "f ", "  ", "  "],
                RowShift {
                    top: 2,
                    bottom: 6,
                    by: 2,
                },
                &[4, 5],
            ),
        ];
        for (lines, shift, rows) in cases {
            let next = rows_snapshot(2, lines);
            let delta = next.delta_from(&base, true).expect("shifted delta");
            assert_eq!(delta.shift, Some(shift), "{lines:?}");
            assert_eq!(
                delta.rows.iter().map(|row| row.index).collect::<Vec<_>>(),
                rows,
                "{lines:?}"
            );
            let mut applied = base.clone();
            applied.apply_delta(delta).unwrap();
            assert_eq!(applied, next, "{lines:?}");

            // Without shifts the same scroll is a row diff or a full screen.
            match next.delta_from(&base, false) {
                Some(delta) => {
                    assert_eq!(delta.shift, None);
                    let mut applied = base.clone();
                    applied.apply_delta(delta).unwrap();
                    assert_eq!(applied, next);
                }
                None => assert!(rows.len() < 4),
            }
        }
    }

    #[test]
    fn repeated_rows_prefer_the_smallest_verified_shift() {
        // `yes`-like rows match many offsets; only one of them retains most rows.
        let base = rows_snapshot(1, &["1 ", "y ", "y ", "2 ", "y ", "y ", "3 ", "y "]);
        let next = rows_snapshot(2, &["y ", "y ", "2 ", "y ", "y ", "3 ", "y ", "y "]);
        let delta = next.delta_from(&base, true).unwrap();
        assert_eq!(
            delta.shift,
            Some(RowShift {
                top: 0,
                bottom: 7,
                by: 1
            })
        );
        assert_eq!(
            delta.rows.iter().map(|row| row.index).collect::<Vec<_>>(),
            [6]
        );
        let mut applied = base.clone();
        applied.apply_delta(delta).unwrap();
        assert_eq!(row_labels(&applied), row_labels(&next));
    }

    #[test]
    fn small_edits_never_shift() {
        let base = rows_snapshot(1, &["a ", "b ", "c ", "d "]);
        let next = rows_snapshot(2, &["a ", "bx", "c ", "d "]);
        let delta = next.delta_from(&base, true).unwrap();
        assert_eq!(delta.shift, None);
        assert_eq!(delta.rows.len(), 1);
    }

    #[test]
    fn shift_wire_field_is_omitted_unless_present() {
        let base = rows_snapshot(1, &["a ", "b ", "c ", "d ", "e "]);
        let unshifted = rows_snapshot(2, &["a ", "bx", "c ", "d ", "e "])
            .delta_from(&base, true)
            .unwrap();
        let json = serde_json::to_value(&unshifted).unwrap();
        assert!(json.get("x").is_none());
        let shifted = rows_snapshot(2, &["b ", "c ", "d ", "e ", "f "])
            .delta_from(&base, true)
            .unwrap();
        let json = serde_json::to_value(&shifted).unwrap();
        assert_eq!(json["x"], serde_json::json!({"t": 0, "e": 5, "n": 1}));
        assert_eq!(
            serde_json::from_value::<ScreenDelta>(json).unwrap(),
            shifted
        );
    }

    #[test]
    fn random_scrolls_and_edits_round_trip_through_deltas() {
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let mut below = |bound: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % bound as u64) as usize
        };
        // Few distinct labels, so rows repeat and hashes match many offsets.
        let label = |n: usize| ["a ", "b ", "y ", "  ", "zz", "y "][n % 6];
        let mut shifted = 0;
        for _ in 0..4000 {
            let rows = 1 + below(12);
            let lines: Vec<&str> = (0..rows).map(|_| label(below(6))).collect();
            let base = rows_snapshot(1, &lines);
            let mut next_lines = lines.clone();
            if rows > 1 {
                let top = below(rows - 1);
                let bottom = top + 2 + below(rows - top - 1);
                let by = 1 + below(bottom - top - 1);
                let region = &mut next_lines[top..bottom];
                if below(2) == 0 {
                    region.rotate_left(by);
                    for line in &mut region[bottom - top - by..] {
                        *line = label(below(6));
                    }
                } else {
                    region.rotate_right(by);
                    for line in &mut region[..by] {
                        *line = label(below(6));
                    }
                }
            }
            for _ in 0..below(3) {
                next_lines[below(rows)] = label(below(6));
            }
            let next = rows_snapshot(2, &next_lines);
            for shifts in [true, false] {
                let Some(delta) = next.delta_from(&base, shifts) else {
                    continue;
                };
                shifted += usize::from(delta.shift.is_some());
                let mut applied = base.clone();
                applied.apply_delta(delta).unwrap();
                assert_eq!(applied, next, "{lines:?} -> {next_lines:?}");
            }
        }
        assert!(shifted > 500, "only {shifted} shifted deltas");
    }

    #[test]
    fn malformed_shifts_are_rejected_without_changing_the_snapshot() {
        for shift in [
            RowShift {
                top: 0,
                bottom: 3,
                by: 1,
            },
            RowShift {
                top: 1,
                bottom: 1,
                by: 1,
            },
            RowShift {
                top: 1,
                bottom: 0,
                by: 1,
            },
            RowShift {
                top: 0,
                bottom: 2,
                by: 0,
            },
            RowShift {
                top: 0,
                bottom: 2,
                by: 2,
            },
            RowShift {
                top: 0,
                bottom: 2,
                by: -2,
            },
            RowShift {
                top: 0,
                bottom: 2,
                by: i16::MIN,
            },
        ] {
            let mut delta = valid_delta(11);
            delta.shift = Some(shift);
            let mut snapshot = delta_snapshot();
            let before = snapshot.clone();
            assert_eq!(
                snapshot.apply_delta(delta),
                Err(ScreenDeltaError::ShiftOutOfBounds { shift, rows: 2 })
            );
            assert_eq!(snapshot, before);
        }
        let mut delta = valid_delta(11);
        delta.shift = Some(RowShift {
            top: 0,
            bottom: 2,
            by: -1,
        });
        let mut snapshot = delta_snapshot();
        snapshot.apply_delta(delta).unwrap();
        assert_eq!(row_labels(&snapshot), ["cd", "xy"]);
    }

    #[test]
    fn compact_cell_style_round_trips_non_defaults() {
        let cell = Cell {
            contents: "λ".into(),
            style: CellStyle::new(
                Some(CellColor::Indexed(1)),
                Some(CellColor::Rgb(Rgb {
                    red: 250,
                    green: 240,
                    blue: 230,
                })),
                true,
                true,
                true,
                true,
                true,
            ),
            selected: true,
            hyperlink: None,
        };
        // Pins Cell and CellStyle's Serialize/Deserialize shapes (format-agnostic: the
        // wire protocol itself uses MessagePack, see
        // protocol::tests::wire_pins_a_styled_cells_exact_messagepack_bytes).
        // A cell is a [contents, style, selected] tuple when styled or selected.
        // Style is a flat [fg, bg, flags] array: fg packs the indexed-color
        // tag (0x0100_0000) with the palette index, bg packs the RGB tag
        // (0x0200_0000) with the r/g/b bytes, and flags is a bitfield
        // (bold|italic|underline|inverse|faint = 0b1_1111).
        let json = serde_json::to_string(&cell).unwrap();
        assert_eq!(serde_json::from_str::<Cell>(&json).unwrap(), cell);
        assert_eq!(json, r#"["λ",142967038510366721,true]"#);
        assert_eq!(serde_json::to_string(&Cell::default()).unwrap(), r#"" ""#);
    }
}
