//! A firmware update that is *addressed* and *acknowledged*, with the blind broadcast path
//! kept underneath it for boards that cannot answer.
//!
//! # Why this exists next to [`crate::fw_update`] rather than replacing it
//!
//! The fielded v4/v5 bootloader never transmits a byte. A host talking to it can only
//! shout the image into the dark, repeat every frame in the hope that one copy lands, and
//! report success because nothing ever said otherwise. [`crate::fw_update`] is that
//! protocol, and it has to stay: a board running that bootloader is exactly the board this
//! update is trying to reach, and it cannot be persuaded to speak.
//!
//! A v6 bootloader answers ([`router_proto::bootloader`]). It reports which chunks it
//! received, CRC-32Cs the programmed bank, and refuses to start an image whose descriptor
//! disagrees with where it is sitting. So the same fleet contains receivers whose best
//! available protocol differs by a factor of everything, and the host cannot know which it
//! is talking to until it has asked.
//!
//! This module is that: recall the fleet, ask, and *then* choose the protocol.
//!
//! ```text
//!   Validate -> Bump ---> Discover --+-- (any silent board) --> LegacyUpload -> Done
//!                                    |
//!                                    +-- (all answered v6) ---> Begin -> Stream
//!                                                                 -> Map <-> Repair
//!                                                                 -> Verify -> Run -> Done
//! ```
//!
//! # The two rules every packet here obeys
//!
//! Both are properties of the transport, not preferences, and breaking either destroys an
//! upload silently rather than loudly:
//!
//! - **`needs_ack: false`.** The worker's ACK wait
//!   (`rs485/worker.rs`, `send_packet`) treats *any* frame whose source matches the
//!   packet's target as the acknowledgement, including a position report that was already
//!   in flight, and blocks the bus for the window when nothing comes. Correlation here is
//!   by the envelope's `seq` trailer instead, which is what actually distinguishes the
//!   reply to this request from the reply to the last one.
//! - **`collateable: false` with an empty `address`.** The outbox keeps only the newest
//!   packet per non-empty `(address, target)` (`rs485/outbox.rs`, `collate`). Several
//!   hundred data frames sharing one address would collapse to exactly the last one.
//!
//! # Non-blocking by construction
//!
//! [`FwSession::tick`] takes the envelopes the caller has already drained and the clock the
//! caller is already reading, and returns. It never sleeps, never waits on the bus, and
//! holds no reference to it between calls -- so the same state machine drives the example
//! CLI, a GUI that must keep painting, and a unit test with a hand-advanced clock and no
//! bus at all.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use router_proto::app_image::{self, BaseSource};
use router_proto::bootloader::{self, BlApp, BlReply, BlSelector, BlVerb};
use router_proto::fw::fw_frame_envelope_trailer;
use router_proto::replies::{classify_reply, Reply};
use router_proto::{crc32c, layout, Envelope};

use crate::fw_update::{self, FwStep, FwUpdateParams};
use crate::rs485::{Packet, Payload, Rs485};

/// The bus, reduced to what an update actually needs from it.
///
/// A trait rather than `&Rs485` so the state machine can be exercised without a serial
/// port, a worker thread or a clock -- which is the only way the ordering guarantees this
/// module exists to provide can be asserted at all.
pub trait FwBus {
    fn transmit(&self, packet: Packet);
    fn outbox_len(&self) -> usize;
    fn clear_outbox(&self);
}

impl FwBus for Rs485 {
    fn transmit(&self, packet: Packet) {
        Rs485::transmit(self, packet);
    }

    fn outbox_len(&self) -> usize {
        Rs485::outbox_len(self)
    }

    fn clear_outbox(&self) {
        Rs485::clear_outbox(self);
    }
}

/// Post-send gap for a control frame. Small: the session sends the next request only once
/// this one has been answered or timed out, so the outbox is empty anyway and this is just
/// enough quiet for the reply not to meet the next transmission head-on.
const CONTROL_GAP_MS: u32 = 2;

/// Which protocol the host is willing to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Ask, then pick. The only setting that is right for a fleet of unknown composition.
    Auto,
    /// The blind broadcast path, without asking. For a bus known to be entirely v4/v5, or
    /// to reproduce exactly what the old Router did.
    LegacyOnly,
    /// The addressed path only. A board that does not answer is left alone rather than
    /// dragging the whole fleet onto the blind path -- but it still counts against the
    /// result unless `silent_is_legacy` is off, which is what turns "did not answer" into
    /// "was not there".
    V6Only,
}

/// How the boards to update are named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Targets {
    /// RS485 ids. What a provisioned installation uses.
    Ids(Vec<i8>),
    /// Provisioning serials, for boards whose id is unknown -- which is the state a board
    /// reaches by power-cycling without an application to hand its id to the bootloader.
    /// The id is learned from the source address of the reply.
    Serials(Vec<u32>),
}

impl Targets {
    fn len(&self) -> usize {
        match self {
            Targets::Ids(ids) => ids.len(),
            Targets::Serials(serials) => serials.len(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FwSessionParams {
    pub mode: Mode,
    pub targets: Targets,
    /// What a board's silence means.
    ///
    /// `true` -- the honest default for a fleet being migrated: a board that does not
    /// answer the control plane is a board running the bootloader that cannot, so the
    /// whole update drops to the blind path. `false` says the fleet is known to be v6 and
    /// a silent board is simply not there, which is what stops one unplugged module from
    /// pulling fifty-three others onto a protocol none of them needs.
    pub silent_is_legacy: bool,
    /// Pacing and framing for the blind path, and for the announce words the addressed
    /// path also uses to recall applications.
    pub legacy: FwUpdateParams,
    /// Data-frame payload size on the addressed path. Must be a multiple of
    /// [`layout::FLASH_GRANULE`] and at most [`layout::BL_CHUNK_MAX`].
    pub chunk_bytes: usize,
    pub data_gap_ms: u32,
    pub status_timeout_ms: u32,
    /// Gap between one board's `begin` and the next board's.
    ///
    /// Not politeness: `begin` erases 53 pages before it answers, so every board addressed
    /// in the same instant answers in the same instant, and on a half-duplex bus that is a
    /// collision rather than 54 replies. Staggering the requests staggers the replies by
    /// the same amount.
    pub begin_stagger_ms: u32,
    /// Time allowed for `begin`. The erase runs one page per main-loop pass, about 1.2 s
    /// for the v6 bank, and the reply comes only when it has finished.
    pub begin_timeout_ms: u32,
    pub map_timeout_ms: u32,
    pub verify_timeout_ms: u32,
    pub run_timeout_ms: u32,
    /// How many times the union of every board's missing chunks is re-broadcast before
    /// giving up and letting `verify` deliver the verdict.
    pub repair_rounds: usize,
    /// How often a bare `"FW"` goes out while boards are being recalled and interrogated.
    ///
    /// A v4/v5 bootloader is resident for 3 s from reset and only an *accepted* frame
    /// extends that. Every frame this module sends while discovering is a `bl` request,
    /// which such a bootloader cannot parse -- so without this, discovery itself is long
    /// enough to let the whole legacy half of the fleet fall back into its application.
    pub keepalive_ms: u32,
    pub run_after: bool,
}

impl Default for FwSessionParams {
    fn default() -> Self {
        Self {
            mode: Mode::Auto,
            targets: Targets::Ids(Vec::new()),
            silent_is_legacy: true,
            legacy: FwUpdateParams::resilient(),
            chunk_bytes: 128,
            data_gap_ms: 6,
            status_timeout_ms: 400,
            begin_stagger_ms: 60,
            begin_timeout_ms: 6_000,
            map_timeout_ms: 800,
            verify_timeout_ms: 2_000,
            run_timeout_ms: 800,
            repair_rounds: 3,
            keepalive_ms: 1_000,
            run_after: true,
        }
    }
}

impl FwSessionParams {
    /// One module on a desk, answering in single-digit milliseconds.
    pub fn bench(id: i8) -> Self {
        Self {
            targets: Targets::Ids(vec![id]),
            status_timeout_ms: 250,
            begin_timeout_ms: 5_000,
            ..Default::default()
        }
    }

    /// A whole column. Longer everything, because 54 boards share one 115200 baud wire.
    pub fn mass(ids: Vec<i8>) -> Self {
        Self {
            targets: Targets::Ids(ids),
            legacy: FwUpdateParams::mass(),
            data_gap_ms: 8,
            status_timeout_ms: 600,
            begin_stagger_ms: 80,
            begin_timeout_ms: 8_000,
            map_timeout_ms: 1_500,
            verify_timeout_ms: 3_000,
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FwSessionError {
    #[error("{0}")]
    Image(#[from] app_image::ImageBaseError),
    #[error("{0}")]
    Upload(#[from] fw_update::FwUpdateError),
    #[error("chunk size {got} must be a non-zero multiple of {granule} and at most {max}")]
    BadChunkSize {
        got: usize,
        granule: usize,
        max: usize,
    },
    #[error("no boards were named")]
    NoTargets,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Validate,
    Bump,
    Discover,
    LegacyUpload,
    Begin,
    Stream,
    Map,
    Repair { round: usize },
    Verify,
    Run,
    Done,
}

/// What a board turned out to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoardKind {
    Unknown,
    /// Answered the v6 control plane.
    V6,
    /// Did not answer, and silence was taken to mean the bootloader that cannot.
    Legacy,
    /// Answered as an application rather than a bootloader: it never took the recall.
    AppRunning,
    /// Did not answer, and silence was taken to mean nothing is there.
    Absent,
}

/// How far a board got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoardState {
    Pending,
    /// `begin` acknowledged: the bank is erased and a session is open.
    Began,
    Streamed,
    /// The chunks this board's bitmap still does not have.
    Missing(Vec<usize>),
    /// Every chunk arrived, by this board's own account.
    Complete,
    Verified {
        crc32: u32,
    },
    VerifyFailed,
    Running,
    /// Silent where a reply was required.
    NoReply(Phase),
    /// The board declined, with its reason.
    Refused(String),
    /// Sent the image on the blind path, where nothing can confirm anything.
    LegacyBlind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Board {
    pub id: i8,
    pub serial: Option<u32>,
    pub uid: Option<[u8; 12]>,
    /// Where this board's bootloader expects an application, from its own `status`.
    pub base: u32,
    /// The largest data frame it will accept.
    pub chunk: u32,
    /// Control-plane version. 6 is the one this module drives.
    pub version: u8,
    /// The application currently installed, if it has a descriptor.
    pub app: Option<BlApp>,
    pub kind: BoardKind,
    pub state: BoardState,
}

#[derive(Debug, Clone)]
pub struct FwProgress {
    pub phase: Phase,
    pub fraction: f32,
    pub detail: String,
    pub boards: Vec<Board>,
    pub packets_queued: usize,
    pub packets_sent: usize,
    pub done: bool,
    pub ok: bool,
}

/// One outstanding request, and what makes a reply count as its answer.
#[derive(Debug, Clone)]
struct Pending {
    board: usize,
    verb: BlVerb,
    seq: u8,
    deadline: Instant,
    attempt: u8,
    /// False while the board is addressed by serial: its id is exactly what the reply is
    /// being asked for, so the source address cannot also be the filter.
    id_known: bool,
}

pub struct FwSession {
    params: FwSessionParams,
    /// The padded image, as programmed and as CRC'd.
    image: Vec<u8>,
    base: u32,
    base_source: BaseSource,
    crc32: u32,
    chunks: usize,
    boards: Vec<Board>,
    asked: Vec<u8>,
    /// Board indices taking part in the addressed path.
    participants: Vec<usize>,

    phase: Phase,
    detail: String,
    ok: bool,
    seq: u8,
    pending: Vec<Pending>,
    cursor: usize,
    discover_pass: u8,
    repair_round: usize,
    stagger_until: Option<Instant>,
    last_legacy_word: Option<Instant>,
    queued_phase_work: bool,
    rebumping: bool,
    queued: usize,
    sent: Arc<AtomicUsize>,
}

impl FwSession {
    /// Establish what the image is and what it is being sent to, before anything is queued.
    ///
    /// The base comes from the image's own descriptor
    /// ([`app_image::image_base`]) rather than from a caller's opinion: the two
    /// application banks overlap, so a `0x08004000` build and a `0x08006000` build are
    /// indistinguishable by inspection and confusing them costs a site visit.
    pub fn new(firmware: &[u8], params: FwSessionParams) -> Result<Self, FwSessionError> {
        if params.targets.len() == 0 {
            return Err(FwSessionError::NoTargets);
        }
        let chunk = params.chunk_bytes;
        if chunk == 0 || chunk > layout::BL_CHUNK_MAX || !chunk.is_multiple_of(layout::FLASH_GRANULE)
        {
            return Err(FwSessionError::BadChunkSize {
                got: chunk,
                granule: layout::FLASH_GRANULE,
                max: layout::BL_CHUNK_MAX,
            });
        }

        let (base, base_source) = app_image::image_base(firmware)?;
        let image = fw_update::prepare_image(firmware, &params.legacy);
        // The same size and framing checks the blind path applies, so an image that cannot
        // be delivered either way is refused once, here, rather than by whichever path the
        // fleet happens to select.
        fw_update::validate(&image, base, &params.legacy)?;

        let crc32 = crc32c(&image);
        let chunks = bootloader::chunk_count(image.len(), chunk);
        let boards: Vec<Board> = match &params.targets {
            Targets::Ids(ids) => ids.iter().map(|id| blank_board(*id, None)).collect(),
            Targets::Serials(serials) => serials
                .iter()
                .map(|serial| blank_board(0, Some(*serial)))
                .collect(),
        };
        let asked = vec![0u8; boards.len()];

        Ok(Self {
            params,
            image,
            base,
            base_source,
            crc32,
            chunks,
            boards,
            asked,
            participants: Vec::new(),
            phase: Phase::Validate,
            detail: String::new(),
            ok: false,
            seq: 0,
            pending: Vec::new(),
            cursor: 0,
            discover_pass: 0,
            repair_round: 0,
            stagger_until: None,
            last_legacy_word: None,
            queued_phase_work: false,
            rebumping: false,
            queued: 0,
            sent: Arc::new(AtomicUsize::new(0)),
        })
    }

    /// The bank this image was linked for, and how that was established.
    pub fn image_base(&self) -> (u32, BaseSource) {
        (self.base, self.base_source)
    }

    /// CRC-32C over the padded image -- what `begin` declares and `verify` must report.
    pub fn image_crc32(&self) -> u32 {
        self.crc32
    }

    pub fn image_len(&self) -> usize {
        self.image.len()
    }

    pub fn chunk_count(&self) -> usize {
        self.chunks
    }

    /// Advance the update. `envelopes` are the frames the caller drained this pass.
    pub fn tick(&mut self, bus: &dyn FwBus, now: Instant, envelopes: &[Envelope]) -> FwProgress {
        self.ingest(envelopes);
        self.expire(bus, now);
        if self.keepalive_due(bus, now) {
            let packet = fw_update::magic_packet(router_proto::fw::FwMagic::Announce, CONTROL_GAP_MS);
            self.send(bus, packet, true, now);
        }
        match self.phase {
            Phase::Validate => self.do_validate(),
            Phase::Bump => self.do_bump(bus, now),
            Phase::Discover => self.do_discover(bus, now),
            Phase::LegacyUpload => self.do_legacy(bus, now),
            Phase::Begin => self.do_begin(bus, now),
            Phase::Stream => self.do_stream(bus, now),
            Phase::Map => self.do_map(bus, now),
            Phase::Repair { .. } => self.do_repair(bus, now),
            Phase::Verify => self.do_verify(bus, now),
            Phase::Run => self.do_run(bus, now),
            Phase::Done => {}
        }
        self.progress()
    }

    /// Abandon the update. The outbox is cleared because most of what is in it is data
    /// frames, and a half-sent image left to drain is worse than one stopped now.
    pub fn abort(&mut self, bus: &dyn FwBus) {
        bus.clear_outbox();
        self.pending.clear();
        self.phase = Phase::Done;
        self.ok = false;
        self.detail = "aborted".into();
    }

    // ---------------------------------------------------------------- replies

    fn ingest(&mut self, envelopes: &[Envelope]) {
        for envelope in envelopes {
            // A frame whose trailer failed decoded to plausible addresses and a plausible
            // body anyway; acting on it is how a corrupted bitmap becomes a repair pass
            // for chunks that already arrived.
            if !envelope.trailer.acceptable() || envelope.source <= 0 {
                continue;
            }
            let reply = classify_reply(&envelope.body);
            let Some(index) = self.match_pending(envelope, &reply) else {
                continue;
            };
            let pending = self.pending.remove(index);
            self.apply(pending, envelope.source, reply);
        }
    }

    fn match_pending(&self, envelope: &Envelope, reply: &Reply) -> Option<usize> {
        let verb = match reply {
            Reply::Bootloader(bl) => bl.verb()?,
            // Only a running application answers with an ACK or a report, and the only
            // question this module asks an application is `status`.
            Reply::Ack(_) | Reply::Report(_) => BlVerb::Status,
            Reply::Other(_) => return None,
        };
        self.pending.iter().position(|pending| {
            pending.verb == verb
                && (!pending.id_known || self.boards[pending.board].id == envelope.source)
                && envelope.trailer.seq().is_none_or(|seq| seq == pending.seq)
        })
    }

    fn apply(&mut self, pending: Pending, source: i8, reply: Reply) {
        let image_len = self.image.len();
        let our_chunk = self.params.chunk_bytes;
        let want_crc = self.crc32;
        let board = &mut self.boards[pending.board];
        // The source address, not the id the board reports: the address it answered on is
        // the one a later unicast has to use, and a board whose two disagree is exactly
        // the board that must not be addressed by the wrong one of them.
        board.id = source;
        match reply {
            Reply::Bootloader(BlReply::Status(status)) => {
                board.version = status.version;
                board.serial = status.serial.or(board.serial);
                board.uid = status.uid;
                board.base = status.base;
                board.chunk = status.chunk;
                board.app = status.app.clone();
                // A bootloader that answers but reports an older control plane is not one
                // this host knows how to drive; the blind path is what it understands.
                board.kind = if bootloader::speaks_v6(&status) {
                    BoardKind::V6
                } else {
                    BoardKind::Legacy
                };
            }
            Reply::Ack(_) | Reply::Report(_) => {
                board.kind = BoardKind::AppRunning;
            }
            Reply::Bootloader(BlReply::Begin { ok, err }) => {
                board.state = if ok {
                    BoardState::Began
                } else {
                    BoardState::Refused(refusal(err))
                };
            }
            Reply::Bootloader(BlReply::Map { chunk, bitmap, .. }) => {
                let missing = missing_indices(&bitmap, chunk as usize, our_chunk, image_len);
                board.state = if missing.is_empty() {
                    BoardState::Complete
                } else {
                    BoardState::Missing(missing)
                };
            }
            Reply::Bootloader(BlReply::Verify { ok, crc32, len }) => {
                // The board's own `ok` is not enough on its own: it compares against what
                // `begin` declared, and a `begin` that was mis-heard would make a board
                // agree with itself about the wrong image.
                board.state = if ok && crc32 == want_crc && len as usize == image_len {
                    BoardState::Verified { crc32 }
                } else {
                    BoardState::VerifyFailed
                };
            }
            Reply::Bootloader(BlReply::Run { ok, err, .. }) => {
                board.state = if ok {
                    BoardState::Running
                } else {
                    BoardState::Refused(refusal(err))
                };
            }
            _ => {}
        }
    }

    fn expire(&mut self, bus: &dyn FwBus, now: Instant) {
        let timed_out: Vec<Pending> = {
            let mut out = Vec::new();
            self.pending.retain(|pending| {
                if now >= pending.deadline {
                    out.push(pending.clone());
                    false
                } else {
                    true
                }
            });
            out
        };
        for pending in timed_out {
            match pending.verb {
                BlVerb::Status => {
                    let silent_is_legacy = self.params.silent_is_legacy;
                    let board = &mut self.boards[pending.board];
                    board.kind = if silent_is_legacy {
                        BoardKind::Legacy
                    } else {
                        BoardKind::Absent
                    };
                }
                // `begin` is the one verb worth retrying blind: it is idempotent (it erases
                // a bank that is already erased) and its reply is the slowest thing on the
                // bus, so a single lost frame either way is the likeliest failure there is.
                BlVerb::Begin if pending.attempt == 0 => {
                    self.send_begin(bus, pending.board, now, 1);
                }
                verb => {
                    let phase = self.phase;
                    self.boards[pending.board].state = BoardState::NoReply(match verb {
                        BlVerb::Begin => Phase::Begin,
                        BlVerb::Map => Phase::Map,
                        BlVerb::Verify => Phase::Verify,
                        BlVerb::Run => Phase::Run,
                        _ => phase,
                    });
                }
            }
        }
    }

    // ----------------------------------------------------------------- phases

    /// Whether this image can reach these boards at all, decided from the image and the
    /// parameters alone.
    ///
    /// Separate from [`FwSession::tick`] so a caller can refuse before it opens a serial
    /// port -- and called *by* the `Validate` phase, so the rule has one home rather than
    /// one per front end.
    pub fn preflight(&self) -> Result<(), String> {
        if self.params.mode == Mode::LegacyOnly && self.base != layout::APP_BASE_LEGACY {
            return Err(self.legacy_base_message());
        }
        Ok(())
    }

    fn do_validate(&mut self) {
        if let Err(why) = self.preflight() {
            self.finish_with(false, why);
            return;
        }
        self.phase = Phase::Bump;
        self.queued_phase_work = false;
    }

    fn do_bump(&mut self, bus: &dyn FwBus, now: Instant) {
        if !self.queued_phase_work {
            let steps = fw_update::announce_steps(&self.params.legacy);
            self.enqueue_steps(bus, &steps, now);
            self.queued_phase_work = true;
            return;
        }
        if bus.outbox_len() > 0 {
            return;
        }
        self.queued_phase_work = false;
        self.cursor = 0;
        if self.params.mode == Mode::LegacyOnly {
            self.take_legacy_path();
        } else {
            self.phase = Phase::Discover;
        }
    }

    fn do_discover(&mut self, bus: &dyn FwBus, now: Instant) {
        if !self.pending.is_empty() {
            return;
        }
        if self.rebumping {
            if bus.outbox_len() > 0 {
                return;
            }
            self.rebumping = false;
        }

        while self.cursor < self.boards.len() {
            let index = self.cursor;
            let due = match self.discover_pass {
                0 => self.asked[index] == 0,
                // Only boards that answered as an application get a second look, and only
                // one: the re-bump is what they missed, not the question.
                _ => self.asked[index] == 1 && self.boards[index].kind == BoardKind::AppRunning,
            };
            if due {
                self.boards[index].kind = BoardKind::Unknown;
                self.send_status(bus, index, now);
                self.cursor += 1;
                return;
            }
            self.cursor += 1;
        }

        if self.discover_pass == 0
            && self
                .boards
                .iter()
                .any(|board| board.kind == BoardKind::AppRunning)
        {
            let steps = fw_update::announce_steps(&self.params.legacy);
            self.enqueue_steps(bus, &steps, now);
            self.rebumping = true;
            self.discover_pass = 1;
            self.cursor = 0;
            return;
        }
        self.decide();
    }

    /// Pick the protocol, now that every board has been given two chances to describe
    /// itself.
    fn decide(&mut self) {
        let any_legacy = self
            .boards
            .iter()
            .any(|board| board.kind == BoardKind::Legacy);
        if self.params.mode == Mode::Auto && any_legacy {
            self.take_legacy_path();
            return;
        }
        self.participants = self
            .boards
            .iter()
            .enumerate()
            .filter(|(_, board)| board.kind == BoardKind::V6)
            .map(|(index, _)| index)
            .collect();
        if self.participants.is_empty() {
            self.finish_with(false, "no board answered the v6 control plane".into());
            return;
        }
        self.phase = Phase::Begin;
        self.cursor = 0;
        self.stagger_until = None;
    }

    fn take_legacy_path(&mut self) {
        if self.base != layout::APP_BASE_LEGACY {
            self.finish_with(false, self.legacy_base_message());
            return;
        }
        for board in &mut self.boards {
            if board.kind != BoardKind::Absent {
                board.state = BoardState::LegacyBlind;
            }
        }
        self.phase = Phase::LegacyUpload;
        self.queued_phase_work = false;
    }

    fn do_legacy(&mut self, bus: &dyn FwBus, now: Instant) {
        if !self.queued_phase_work {
            let params = FwUpdateParams {
                run_after: self.params.run_after,
                ..self.params.legacy.clone()
            };
            // The announce phases already went out during Bump; only the destructive half
            // is queued here.
            match fw_update::upload_steps(&self.image, self.base, &params) {
                Ok(steps) => {
                    self.enqueue_steps(bus, &steps, now);
                    self.queued_phase_work = true;
                }
                Err(error) => self.finish_with(false, error.to_string()),
            }
            return;
        }
        if bus.outbox_len() == 0 {
            self.finish();
        }
    }

    fn do_begin(&mut self, bus: &dyn FwBus, now: Instant) {
        if self.cursor < self.participants.len() {
            if self.stagger_until.is_some_and(|until| now < until) {
                return;
            }
            let index = self.participants[self.cursor];
            self.cursor += 1;
            if self.boards[index].base != self.base {
                let board = &mut self.boards[index];
                board.state = BoardState::Refused(format!(
                    "bootloader expects an application at 0x{:08X}; this image is linked for \
                     0x{:08X}",
                    board.base, self.base
                ));
                return;
            }
            self.send_begin(bus, index, now, 0);
            self.stagger_until =
                Some(now + Duration::from_millis(u64::from(self.params.begin_stagger_ms)));
            return;
        }
        if !self.pending.is_empty() {
            return;
        }
        if !self
            .boards
            .iter()
            .any(|board| board.state == BoardState::Began)
        {
            self.finish_with(false, "no board opened a session".into());
            return;
        }
        self.phase = Phase::Stream;
        self.queued_phase_work = false;
    }

    fn do_stream(&mut self, bus: &dyn FwBus, now: Instant) {
        if !self.queued_phase_work {
            for index in 0..self.chunks {
                self.send_chunk(bus, index, now);
            }
            self.queued_phase_work = true;
            return;
        }
        if bus.outbox_len() > 0 {
            return;
        }
        for index in self.participants.clone() {
            if self.boards[index].state == BoardState::Began {
                self.boards[index].state = BoardState::Streamed;
            }
        }
        self.phase = Phase::Map;
        self.cursor = 0;
    }

    fn do_map(&mut self, bus: &dyn FwBus, now: Instant) {
        if !self.pending.is_empty() {
            return;
        }
        while self.cursor < self.participants.len() {
            let index = self.participants[self.cursor];
            self.cursor += 1;
            if needs_map(&self.boards[index].state) {
                self.send_map(bus, index, now);
                return;
            }
        }
        if !self.union_missing().is_empty() && self.repair_round < self.params.repair_rounds {
            self.phase = Phase::Repair {
                round: self.repair_round,
            };
            self.queued_phase_work = false;
            return;
        }
        self.phase = Phase::Verify;
        self.cursor = 0;
    }

    fn do_repair(&mut self, bus: &dyn FwBus, now: Instant) {
        if !self.queued_phase_work {
            for index in self.union_missing() {
                self.send_chunk(bus, index, now);
            }
            self.queued_phase_work = true;
            return;
        }
        if bus.outbox_len() > 0 {
            return;
        }
        self.repair_round += 1;
        self.phase = Phase::Map;
        self.cursor = 0;
    }

    fn do_verify(&mut self, bus: &dyn FwBus, now: Instant) {
        if !self.pending.is_empty() {
            return;
        }
        while self.cursor < self.participants.len() {
            let index = self.participants[self.cursor];
            self.cursor += 1;
            if needs_verify(&self.boards[index].state) {
                self.send_verify(bus, index, now);
                return;
            }
        }
        if !self.params.run_after {
            self.finish();
            return;
        }
        self.phase = Phase::Run;
        self.cursor = 0;
    }

    fn do_run(&mut self, bus: &dyn FwBus, now: Instant) {
        if !self.pending.is_empty() {
            return;
        }
        while self.cursor < self.participants.len() {
            let index = self.participants[self.cursor];
            self.cursor += 1;
            if matches!(self.boards[index].state, BoardState::Verified { .. }) {
                self.send_run(bus, index, now);
                return;
            }
        }
        self.finish();
    }

    // --------------------------------------------------------------- requests

    fn send_status(&mut self, bus: &dyn FwBus, index: usize, now: Instant) {
        let seq = self.next_seq();
        let (target, selector, id_known) = match self.params.targets {
            Targets::Ids(_) => (self.boards[index].id, BlSelector::None, true),
            // A broadcast is answered by nobody unless it names one board; the serial is
            // that name, and the id comes back in the reply's source address.
            Targets::Serials(_) => (
                router_proto::BROADCAST,
                BlSelector::Serial(self.boards[index].serial.unwrap_or(0)),
                false,
            ),
        };
        let bytes = bootloader::status(target, selector, seq);
        self.send(bus, control_packet(target, bytes), false, now);
        self.asked[index] += 1;
        self.push_pending(index, BlVerb::Status, seq, now, self.params.status_timeout_ms, 0, id_known);
    }

    fn send_begin(&mut self, bus: &dyn FwBus, index: usize, now: Instant, attempt: u8) {
        let seq = self.next_seq();
        let target = self.boards[index].id;
        // The base is stated rather than defaulted. The bootloader would fill in its own,
        // which is the same value here -- but a frame that says which bank it means cannot
        // be misread by a bootloader whose default ever changes.
        let bytes = bootloader::begin(
            target,
            self.image.len() as u32,
            self.crc32,
            self.params.chunk_bytes as u32,
            Some(self.base),
            seq,
        );
        self.send(bus, control_packet(target, bytes), false, now);
        self.push_pending(
            index,
            BlVerb::Begin,
            seq,
            now,
            self.params.begin_timeout_ms,
            attempt,
            true,
        );
    }

    fn send_map(&mut self, bus: &dyn FwBus, index: usize, now: Instant) {
        let seq = self.next_seq();
        let target = self.boards[index].id;
        let bytes = bootloader::map_request(target, None, seq);
        self.send(bus, control_packet(target, bytes), false, now);
        self.push_pending(index, BlVerb::Map, seq, now, self.params.map_timeout_ms, 0, true);
    }

    fn send_verify(&mut self, bus: &dyn FwBus, index: usize, now: Instant) {
        let seq = self.next_seq();
        let target = self.boards[index].id;
        let bytes = bootloader::verify(target, seq);
        self.send(bus, control_packet(target, bytes), false, now);
        self.push_pending(
            index,
            BlVerb::Verify,
            seq,
            now,
            self.params.verify_timeout_ms,
            0,
            true,
        );
    }

    fn send_run(&mut self, bus: &dyn FwBus, index: usize, now: Instant) {
        let seq = self.next_seq();
        let target = self.boards[index].id;
        let bytes = bootloader::run(target, seq);
        self.send(bus, control_packet(target, bytes), false, now);
        self.push_pending(index, BlVerb::Run, seq, now, self.params.run_timeout_ms, 0, true);
    }

    /// One data frame, broadcast: 54 boards take the same image from the same transmission,
    /// which is the whole reason the addressed path is still worth having over 54 unicast
    /// uploads.
    fn send_chunk(&mut self, bus: &dyn FwBus, index: usize, now: Instant) {
        let chunk = self.params.chunk_bytes;
        let start = index * chunk;
        let end = (start + chunk).min(self.image.len());
        if start >= end {
            return;
        }
        let seq = self.next_seq();
        let bytes = fw_frame_envelope_trailer(start as u32, &self.image[start..end], seq);
        let mut packet = control_packet(router_proto::BROADCAST, bytes);
        packet.custom_wait_time_ms = Some(self.params.data_gap_ms);
        // A data frame is one of the words a v4/v5 bootloader can parse, so it extends
        // residency exactly as an announce would.
        self.send(bus, packet, true, now);
    }

    fn enqueue_steps(&mut self, bus: &dyn FwBus, steps: &[FwStep], now: Instant) {
        let packets: Vec<(Packet, bool)> = steps
            .iter()
            .map(|step| (fw_update::step_packet(*step, &self.image), parseable_by_legacy(*step)))
            .collect();
        for (packet, legacy_word) in packets {
            self.send(bus, packet, legacy_word, now);
        }
    }

    fn send(&mut self, bus: &dyn FwBus, mut packet: Packet, legacy_word: bool, now: Instant) {
        let counter = self.sent.clone();
        packet.on_sent = Some(Box::new(move || {
            counter.fetch_add(1, Ordering::Relaxed);
        }));
        self.queued += 1;
        if legacy_word {
            self.last_legacy_word = Some(now);
        }
        bus.transmit(packet);
    }

    #[allow(clippy::too_many_arguments)]
    fn push_pending(
        &mut self,
        board: usize,
        verb: BlVerb,
        seq: u8,
        now: Instant,
        timeout_ms: u32,
        attempt: u8,
        id_known: bool,
    ) {
        self.pending.push(Pending {
            board,
            verb,
            seq,
            deadline: now + Duration::from_millis(u64::from(timeout_ms)),
            attempt,
            id_known,
        });
    }

    fn next_seq(&mut self) -> u8 {
        self.seq = self.seq.wrapping_add(1);
        self.seq
    }

    // ----------------------------------------------------------------- upkeep

    /// Whether a bare `"FW"` is owed.
    ///
    /// Only while boards are being recalled, interrogated and opened -- after `begin` every
    /// participating bootloader is held by its own open session, and before `begin` a
    /// legacy board hears nothing else it can parse. Not during `Validate`, which decides
    /// whether this update happens at all and must be able to refuse without having put a
    /// byte on the bus. Suppressed while the outbox is non-empty, because a queue that is
    /// still draining is already keeping the bus busy with words of exactly this kind.
    fn keepalive_due(&self, bus: &dyn FwBus, now: Instant) -> bool {
        if !matches!(
            self.phase,
            Phase::Bump | Phase::Discover | Phase::Begin
        ) || bus.outbox_len() > 0
        {
            return false;
        }
        let period = Duration::from_millis(u64::from(self.params.keepalive_ms.max(1)));
        self.last_legacy_word
            .is_none_or(|last| now.duration_since(last) >= period)
    }

    fn union_missing(&self) -> Vec<usize> {
        let mut union: BTreeSet<usize> = BTreeSet::new();
        for board in &self.boards {
            if let BoardState::Missing(indices) = &board.state {
                union.extend(indices.iter().copied());
            }
        }
        let mut ordered: Vec<usize> = union.into_iter().collect();
        // Chunk 0 first, in every pass: it carries the vector table, and a bank whose
        // first double-word is still erased is one a bootloader will refuse to start
        // however complete the rest of it is.
        if let Some(position) = ordered.iter().position(|index| *index == 0) {
            ordered.remove(position);
            ordered.insert(0, 0);
        }
        ordered
    }

    fn legacy_base_message(&self) -> String {
        format!(
            "this image is linked for 0x{:08X}, and the blind broadcast path only reaches \
             bootloaders whose application starts at 0x{:08X}; build the *_legacy_base target \
             for these boards",
            self.base,
            layout::APP_BASE_LEGACY
        )
    }

    fn finish(&mut self) {
        let ok = self.boards.iter().all(|board| {
            board.kind == BoardKind::Absent
                || matches!(
                    board.state,
                    BoardState::Running | BoardState::Verified { .. } | BoardState::LegacyBlind
                )
        });
        let detail = if ok {
            format!("{} boards updated", self.boards.len())
        } else {
            let bad = self
                .boards
                .iter()
                .filter(|board| {
                    board.kind != BoardKind::Absent
                        && !matches!(
                            board.state,
                            BoardState::Running
                                | BoardState::Verified { .. }
                                | BoardState::LegacyBlind
                        )
                })
                .count();
            format!("{bad} of {} boards did not finish", self.boards.len())
        };
        self.finish_with(ok, detail);
    }

    fn finish_with(&mut self, ok: bool, detail: String) {
        self.phase = Phase::Done;
        self.ok = ok;
        self.detail = detail;
        self.pending.clear();
    }

    fn progress(&self) -> FwProgress {
        FwProgress {
            phase: self.phase,
            fraction: self.fraction(),
            detail: if self.detail.is_empty() {
                phase_label(self.phase).to_string()
            } else {
                self.detail.clone()
            },
            boards: self.boards.clone(),
            packets_queued: self.queued,
            packets_sent: self.sent.load(Ordering::Relaxed),
            done: self.phase == Phase::Done,
            ok: self.phase == Phase::Done && self.ok,
        }
    }

    /// A 0..1 estimate for a progress bar, weighted so the two phases that take minutes --
    /// streaming, and the blind path's whole broadcast -- are most of it. Within those it
    /// tracks packets actually sent, which is the only measure of an unacknowledged
    /// broadcast that means anything.
    fn fraction(&self) -> f32 {
        let sent = self.sent.load(Ordering::Relaxed) as f32;
        let queued = self.queued.max(1) as f32;
        match self.phase {
            Phase::Validate => 0.0,
            Phase::Bump => 0.02 + 0.08 * (sent / queued),
            Phase::Discover => 0.15,
            Phase::LegacyUpload | Phase::Stream => 0.2 + 0.6 * (sent / queued),
            Phase::Map => 0.82,
            Phase::Repair { round } => {
                0.84 + 0.04 * (round as f32 / self.params.repair_rounds.max(1) as f32)
            }
            Phase::Begin => 0.2,
            Phase::Verify => 0.92,
            Phase::Run => 0.97,
            Phase::Done => 1.0,
        }
    }
}

fn blank_board(id: i8, serial: Option<u32>) -> Board {
    Board {
        id,
        serial,
        uid: None,
        base: 0,
        chunk: 0,
        version: 0,
        app: None,
        kind: BoardKind::Unknown,
        state: BoardState::Pending,
    }
}

fn control_packet(target: i8, bytes: Vec<u8>) -> Packet {
    Packet {
        payload: Payload::Rendered(bytes),
        target,
        address: String::new(),
        needs_ack: false,
        collateable: false,
        custom_wait_time_ms: Some(CONTROL_GAP_MS),
        on_sent: None,
    }
}

/// Whether a v4/v5 bootloader can parse this step, and so whether it extends residency.
///
/// `"FW!KC79"` cannot be: that bootloader reads an announce into a 3-byte buffer, so a
/// 7-byte string is a format error rather than a keepalive.
fn parseable_by_legacy(step: FwStep) -> bool {
    !matches!(
        step,
        FwStep::Magic {
            magic: router_proto::fw::FwMagic::AnnounceLong,
            ..
        }
    )
}

fn needs_map(state: &BoardState) -> bool {
    matches!(
        state,
        BoardState::Began
            | BoardState::Streamed
            | BoardState::Missing(_)
            // A repair round is another chance to ask a board whose last bitmap went
            // missing on the way back.
            | BoardState::NoReply(Phase::Map)
    )
}

/// `verify` is the only answer that settles what a board actually holds, so it is asked of
/// every board that got as far as opening a session -- including one whose bitmap said
/// chunks were missing, and one whose bitmap never arrived. A lost `map` reply and a lost
/// data frame are indistinguishable from here, and only one of them is a real failure.
fn needs_verify(state: &BoardState) -> bool {
    matches!(
        state,
        BoardState::Began
            | BoardState::Streamed
            | BoardState::Missing(_)
            | BoardState::Complete
            | BoardState::NoReply(Phase::Map)
    )
}

fn refusal(err: Option<u8>) -> String {
    bootloader::error_name(err.unwrap_or(bootloader::err::NONE)).to_string()
}

/// Translate a board's bitmap into indices of *our* chunks.
///
/// The two granularities are normally the same -- `map` is asked without an override, so a
/// board reports at the size `begin` declared. They are reconciled anyway because a board
/// is entitled to answer at its own granularity, and reading its bitmap at the wrong scale
/// would repair the wrong chunks while reporting success.
fn missing_indices(
    bitmap: &[u8],
    reported_chunk: usize,
    our_chunk: usize,
    len: usize,
) -> Vec<usize> {
    if our_chunk == 0 || len == 0 {
        return Vec::new();
    }
    let reported = if reported_chunk == 0 {
        our_chunk
    } else {
        reported_chunk
    };
    let count = bootloader::chunk_count(len, reported);
    let mut out: BTreeSet<usize> = BTreeSet::new();
    for index in bootloader::missing_chunks(bitmap, count) {
        let start = index * reported;
        let end = ((index + 1) * reported).min(len);
        if start >= end {
            continue;
        }
        for ours in start / our_chunk..=(end - 1) / our_chunk {
            out.insert(ours);
        }
    }
    out.into_iter().collect()
}

fn phase_label(phase: Phase) -> &'static str {
    match phase {
        Phase::Validate => "checking the image",
        Phase::Bump => "recalling boards into their bootloaders",
        Phase::Discover => "asking each board what it is",
        Phase::LegacyUpload => "broadcasting (blind path)",
        Phase::Begin => "erasing",
        Phase::Stream => "streaming",
        Phase::Map => "reading received-chunk maps",
        Phase::Repair { .. } => "repairing gaps",
        Phase::Verify => "verifying",
        Phase::Run => "starting the application",
        Phase::Done => "done",
    }
}

/// A bus that records what was sent and hands it back, for tests that have no serial port.
///
/// Packets are treated as sent the instant they are handed over -- `outbox_len` is always
/// zero -- so a test advances the clock rather than waiting for a queue to drain.
#[cfg(test)]
pub(crate) struct MockBus {
    sent: std::sync::Mutex<Vec<SentPacket>>,
    cleared: std::sync::atomic::AtomicUsize,
}

#[cfg(test)]
#[derive(Debug, Clone)]
pub(crate) struct SentPacket {
    pub bytes: Vec<u8>,
    pub target: i8,
    pub needs_ack: bool,
    pub collateable: bool,
    pub address: String,
    pub wait_ms: Option<u32>,
}

#[cfg(test)]
impl MockBus {
    pub fn new() -> Self {
        Self {
            sent: std::sync::Mutex::new(Vec::new()),
            cleared: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    pub fn sent(&self) -> Vec<SentPacket> {
        self.sent.lock().unwrap().clone()
    }

    pub fn len(&self) -> usize {
        self.sent.lock().unwrap().len()
    }

    pub fn clears(&self) -> usize {
        self.cleared.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
impl FwBus for MockBus {
    fn transmit(&self, packet: Packet) {
        let Packet {
            payload,
            target,
            address,
            needs_ack,
            collateable,
            custom_wait_time_ms,
            on_sent,
        } = packet;
        let bytes = match payload {
            Payload::Rendered(bytes) => bytes,
            Payload::Lazy(render) => render(),
        };
        self.sent.lock().unwrap().push(SentPacket {
            bytes,
            target,
            needs_ack,
            collateable,
            address,
            wait_ms: custom_wait_time_ms,
        });
        if let Some(on_sent) = on_sent {
            on_sent();
        }
    }

    fn outbox_len(&self) -> usize {
        0
    }

    fn clear_outbox(&self) {
        self.cleared.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use router_proto::envelope::{decode_envelope, encode_reply_trailer};
    use router_proto::value::{key, map};
    use router_proto::Value;

    const CHUNK: usize = 128;

    /// An image that states its own base, the way a v6-era build does.
    fn image_for(base: u32, bytes: usize) -> Vec<u8> {
        let mut image = vec![0x5Au8; bytes.max(0x400)];
        image[..4].copy_from_slice(&layout::RAM_END.to_le_bytes());
        image[4..8].copy_from_slice(&((base + 0x241) | 1).to_le_bytes());
        let at = layout::APP_DESCRIPTOR_OFFSET;
        image[at..at + 8].copy_from_slice(layout::APP_DESCRIPTOR_MAGIC);
        image[at + 8..at + 12].copy_from_slice(&base.to_le_bytes());
        image[at + 12..at + 16].copy_from_slice(&0u32.to_le_bytes());
        image.truncate(bytes);
        image
    }

    /// A pre-descriptor image: only its reset vector says where it belongs.
    fn legacy_image(bytes: usize) -> Vec<u8> {
        let mut image = vec![0x5Au8; bytes];
        image[..4].copy_from_slice(&layout::RAM_END.to_le_bytes());
        image[4..8].copy_from_slice(&((layout::APP_BASE_LEGACY + 0x241) | 1).to_le_bytes());
        image
    }

    fn params(ids: Vec<i8>) -> FwSessionParams {
        FwSessionParams {
            targets: Targets::Ids(ids),
            chunk_bytes: CHUNK,
            ..Default::default()
        }
    }

    struct Harness {
        session: FwSession,
        bus: MockBus,
        now: Instant,
        inbox: Vec<Envelope>,
        /// Every packet, with the moment it was handed to the bus.
        timeline: Vec<(Instant, SentPacket)>,
        seen: usize,
    }

    impl Harness {
        fn new(firmware: &[u8], params: FwSessionParams) -> Self {
            Self {
                session: FwSession::new(firmware, params).expect("session"),
                bus: MockBus::new(),
                now: Instant::now(),
                inbox: Vec::new(),
                timeline: Vec::new(),
                seen: 0,
            }
        }

        fn tick(&mut self) -> FwProgress {
            let inbox = std::mem::take(&mut self.inbox);
            let progress = self.session.tick(&self.bus, self.now, &inbox);
            let sent = self.bus.sent();
            for packet in &sent[self.seen..] {
                self.timeline.push((self.now, packet.clone()));
            }
            self.seen = sent.len();
            progress
        }

        fn advance(&mut self, ms: u64) {
            self.now += Duration::from_millis(ms);
        }

        /// Run until `Done`, or until the step budget runs out.
        fn run(&mut self, step_ms: u64, mut respond: impl FnMut(&mut Self)) -> FwProgress {
            let mut progress = self.tick();
            for _ in 0..40_000 {
                if progress.done {
                    break;
                }
                respond(self);
                self.advance(step_ms);
                progress = self.tick();
            }
            progress
        }

        /// The last request sent to `target`, decoded.
        fn last_request(&self, target: i8) -> Option<(Envelope, u8)> {
            self.timeline
                .iter()
                .rev()
                .find(|(_, packet)| packet.target == target)
                .and_then(|(_, packet)| {
                    let envelope = decode_envelope(&packet.bytes).ok()?;
                    let seq = envelope.trailer.seq()?;
                    Some((envelope, seq))
                })
        }

        /// The verb of the outstanding request to `target`, if it is a `bl` one.
        fn outstanding(&self, target: i8) -> Option<(String, u8)> {
            let (envelope, seq) = self.last_request(target)?;
            let Value::Map(entries) = &envelope.body else {
                return None;
            };
            let (_, inner) = entries.iter().find(|(k, _)| k.as_str() == Some("bl"))?;
            let Value::Map(fields) = inner else {
                return None;
            };
            let verb = fields
                .iter()
                .find(|(k, _)| k.as_str() == Some("q"))?
                .1
                .as_str()?
                .to_string();
            Some((verb, seq))
        }

        fn reply(&mut self, source: i8, seq: u8, fields: Vec<(Value, Value)>) {
            let body = map(vec![(key("bl"), map(fields))]);
            let bytes = encode_reply_trailer(source, &body, seq);
            self.inbox.push(decode_envelope(&bytes).unwrap());
        }

        fn ack(&mut self, source: i8, seq: u8) {
            let bytes = encode_reply_trailer(source, &Value::Boolean(true), seq);
            self.inbox.push(decode_envelope(&bytes).unwrap());
        }

        fn board(&self, id: i8) -> &Board {
            self.session
                .boards
                .iter()
                .find(|board| board.id == id)
                .expect("board")
        }
    }

    fn status_fields(id: i8, base: u32) -> Vec<(Value, Value)> {
        vec![
            (key("q"), Value::from("status")),
            (key("v"), Value::from(6)),
            (key("id"), Value::from(id)),
            (key("src"), Value::from("handoff")),
            (key("s"), Value::from(73_000 + id as u32)),
            (key("base"), Value::from(base)),
            (key("cap"), Value::from(layout::app_bank_bytes(base) as u32)),
            (key("chunk"), Value::from(layout::BL_CHUNK_MAX as u32)),
            (key("st"), Value::from(3)),
        ]
    }

    /// Answer whatever is outstanding for `id` the way a healthy v6 board would.
    fn answer_v6(h: &mut Harness, id: i8, base: u32, bitmap: Option<Vec<u8>>) {
        let Some((verb, seq)) = h.outstanding(id) else {
            return;
        };
        match verb.as_str() {
            "status" => {
                let fields = status_fields(id, base);
                h.reply(id, seq, fields);
            }
            "begin" => h.reply(
                id,
                seq,
                vec![(key("q"), Value::from("begin")), (key("ok"), Value::from(true))],
            ),
            "map" => {
                let chunks = h.session.chunks;
                let full = vec![0xFFu8; chunks.div_ceil(8)];
                h.reply(
                    id,
                    seq,
                    vec![
                        (key("q"), Value::from("map")),
                        (key("chunk"), Value::from(CHUNK as u32)),
                        (key("len"), Value::from(h.session.image.len() as u32)),
                        (key("map"), Value::Binary(bitmap.unwrap_or(full))),
                    ],
                );
            }
            "verify" => {
                let crc = h.session.crc32;
                let len = h.session.image.len() as u32;
                h.reply(
                    id,
                    seq,
                    vec![
                        (key("q"), Value::from("verify")),
                        (key("ok"), Value::from(true)),
                        (key("crc"), Value::from(crc)),
                        (key("len"), Value::from(len)),
                    ],
                );
            }
            "run" => h.reply(
                id,
                seq,
                vec![
                    (key("q"), Value::from("run")),
                    (key("ok"), Value::from(true)),
                    (key("base"), Value::from(base)),
                ],
            ),
            _ => {}
        }
    }

    fn is_data_frame(packet: &SentPacket) -> bool {
        decode_envelope(&packet.bytes)
            .ok()
            .is_some_and(|envelope| matches!(&envelope.body, Value::Map(entries)
                if entries.first().is_some_and(|(k, _)| k.as_str().is_none())))
    }

    fn data_offsets(h: &Harness) -> Vec<u32> {
        h.timeline
            .iter()
            .filter(|(_, packet)| is_data_frame(packet))
            .filter_map(|(_, packet)| {
                let envelope = decode_envelope(&packet.bytes).ok()?;
                let Value::Map(entries) = &envelope.body else {
                    return None;
                };
                u32::try_from(entries.first()?.0.as_u64()?).ok()
            })
            .collect()
    }

    // ------------------------------------------------------------- discovery

    #[test]
    fn discovery_classifies_each_board_from_what_it_answers() {
        let firmware = image_for(layout::APP_BASE, 512);
        let mut h = Harness::new(
            &firmware,
            FwSessionParams {
                silent_is_legacy: false,
                ..params(vec![1, 2, 3, 4])
            },
        );
        // Board 1 answers as a v6 bootloader, 2 as a running application (twice, so the
        // re-bump does not rescue it), 3 stays silent, 4 answers v6 only after the re-bump.
        let mut appeared = false;
        h.run(60, |h| {
            answer_v6(h, 1, layout::APP_BASE, None);
            if let Some((verb, seq)) = h.outstanding(2) {
                if verb == "status" {
                    h.ack(2, seq);
                }
            }
            if appeared {
                answer_v6(h, 4, layout::APP_BASE, None);
            }
            if let Some((verb, _)) = h.outstanding(4) {
                if verb == "status" {
                    appeared = true;
                }
            }
        });

        assert_eq!(h.board(1).kind, BoardKind::V6);
        assert_eq!(h.board(1).serial, Some(73_001));
        assert_eq!(h.board(1).base, layout::APP_BASE);
        assert_eq!(h.board(2).kind, BoardKind::AppRunning);
        assert_eq!(h.board(3).kind, BoardKind::Absent, "silence, not a bootloader");
        assert_eq!(h.board(4).kind, BoardKind::V6);
        // Exactly one board got a second look, and it is the one that answered as an app.
        assert_eq!(h.session.asked[1], 2);
        assert_eq!(h.session.asked[0], 1);
    }

    /// The gap a legacy bootloader cannot survive. Discovery is the phase where it applies:
    /// every request in it is a `bl` frame that such a bootloader reads as a format error.
    #[test]
    fn discovery_never_goes_quiet_long_enough_for_a_legacy_bootloader_to_leave() {
        let firmware = legacy_image(512);
        let ids: Vec<i8> = (1..=12).collect();
        let mut h = Harness::new(&firmware, params(ids));
        // Nobody answers: the longest possible discovery, one full status timeout per board.
        h.run(50, |_| {});

        const RESIDENCY_FLOOR_MS: u128 = 3_000;
        let words: Vec<Instant> = h
            .timeline
            .iter()
            .filter(|(_, packet)| {
                decode_envelope(&packet.bytes)
                    .ok()
                    .is_some_and(|envelope| envelope.body.as_str() == Some("FW"))
                    || is_data_frame(packet)
            })
            .map(|(at, _)| *at)
            .collect();
        assert!(words.len() > 5, "only {} parseable words", words.len());

        let mut worst = 0u128;
        for pair in words.windows(2) {
            worst = worst.max(pair[1].duration_since(pair[0]).as_millis());
        }
        assert!(
            worst < RESIDENCY_FLOOR_MS,
            "a legacy bootloader would go {worst} ms without a word it can parse"
        );
    }

    #[test]
    fn serial_targets_learn_the_id_from_the_reply() {
        let firmware = image_for(layout::APP_BASE, 512);
        let mut h = Harness::new(
            &firmware,
            FwSessionParams {
                targets: Targets::Serials(vec![73_007]),
                chunk_bytes: CHUNK,
                ..Default::default()
            },
        );
        for _ in 0..8 {
            if h.outstanding(router_proto::BROADCAST)
                .is_some_and(|(verb, _)| verb == "status")
            {
                break;
            }
            h.advance(50);
            h.tick();
        }
        // The request is a broadcast carrying the serial, because the id is the unknown.
        let (envelope, seq) = h.last_request(router_proto::BROADCAST).expect("broadcast");
        let Value::Map(entries) = &envelope.body else {
            panic!("not a map")
        };
        let (_, inner) = entries
            .iter()
            .find(|(k, _)| k.as_str() == Some("bl"))
            .expect("bl");
        let Value::Map(fields) = inner else {
            panic!("bl is not a map")
        };
        assert_eq!(
            fields
                .iter()
                .find(|(k, _)| k.as_str() == Some("s"))
                .and_then(|(_, v)| v.as_u64()),
            Some(73_007)
        );

        h.reply(7, seq, status_fields(7, layout::APP_BASE));
        h.advance(10);
        h.tick();
        assert_eq!(h.session.boards[0].id, 7, "the id came from the reply");
        assert_eq!(h.session.boards[0].kind, BoardKind::V6);
    }

    // ------------------------------------------------------------ refusals

    /// The failure this whole two-base mess exists to prevent, caught before the bus is
    /// touched at all.
    #[test]
    fn a_new_base_image_on_the_blind_path_is_refused_before_a_single_packet() {
        let firmware = image_for(layout::APP_BASE, 512);
        let mut h = Harness::new(
            &firmware,
            FwSessionParams {
                mode: Mode::LegacyOnly,
                ..params(vec![1, 2])
            },
        );
        let progress = h.tick();
        assert!(progress.done && !progress.ok);
        assert_eq!(h.bus.len(), 0, "nothing was queued");
        assert!(
            progress.detail.contains("_legacy_base"),
            "the message must name the build to use: {}",
            progress.detail
        );
    }

    /// The same refusal after discovery, which is where a mixed fleet reaches it. The
    /// announce words have gone out by then -- they are harmless -- but no erase and no
    /// data frame has.
    #[test]
    fn a_legacy_fleet_and_a_new_base_image_is_refused_before_any_data_frame() {
        let firmware = image_for(layout::APP_BASE, 512);
        let mut h = Harness::new(&firmware, params(vec![1, 2, 3]));
        let progress = h.run(50, |_| {});
        assert!(progress.done && !progress.ok);
        assert!(progress.detail.contains("_legacy_base"), "{}", progress.detail);
        assert_eq!(data_offsets(&h), Vec::<u32>::new(), "no image bytes were sent");
        let erases = h
            .timeline
            .iter()
            .filter(|(_, packet)| {
                decode_envelope(&packet.bytes)
                    .ok()
                    .is_some_and(|envelope| envelope.body.as_str() == Some("ER"))
            })
            .count();
        assert_eq!(erases, 0, "nothing was erased");
    }

    #[test]
    fn a_board_whose_bank_disagrees_with_the_image_is_refused_and_the_rest_proceed() {
        let firmware = image_for(layout::APP_BASE, 512);
        let mut h = Harness::new(&firmware, params(vec![1, 2]));
        let progress = h.run(50, |h| {
            answer_v6(h, 1, layout::APP_BASE, None);
            answer_v6(h, 2, layout::APP_BASE_LEGACY, None);
        });
        assert!(progress.done);
        assert!(!progress.ok, "one board was refused");
        assert_eq!(h.board(1).state, BoardState::Running);
        let BoardState::Refused(why) = &h.board(2).state else {
            panic!("board 2 is {:?}", h.board(2).state)
        };
        assert!(why.contains("0x08006000"), "{why}");
        // A refused board is refused before anything is asked of it.
        assert_eq!(h.outstanding(2).map(|(verb, _)| verb), Some("status".into()));
    }

    // ------------------------------------------------------------ happy path

    #[test]
    fn an_all_v6_fleet_begins_streams_maps_verifies_and_runs() {
        let firmware = image_for(layout::APP_BASE, 1_024);
        let mut h = Harness::new(&firmware, params(vec![1, 2]));
        let progress = h.run(40, |h| {
            answer_v6(h, 1, layout::APP_BASE, None);
            answer_v6(h, 2, layout::APP_BASE, None);
        });

        assert!(progress.done && progress.ok, "{}", progress.detail);
        assert_eq!(h.board(1).state, BoardState::Running);
        assert_eq!(h.board(2).state, BoardState::Running);

        // `run` is unicast per board and sent once each. Broadcasting it would start both
        // applications from one frame, but nothing would then say whether either did.
        for target in [1i8, 2i8] {
            let runs = h
                .timeline
                .iter()
                .filter(|(_, packet)| packet.target == target)
                .filter(|(_, packet)| {
                    decode_envelope(&packet.bytes)
                        .ok()
                        .is_some_and(|envelope| body_verb(&envelope) == Some("run".into()))
                })
                .count();
            assert_eq!(runs, 1, "board {target} was asked to run {runs} times");
        }

        // Every chunk exactly once, in order, at the configured size and gap.
        let offsets = data_offsets(&h);
        let expected: Vec<u32> = (0..h.session.chunks).map(|i| (i * CHUNK) as u32).collect();
        assert_eq!(offsets, expected);
        assert_eq!(offsets.len(), 8, "1024 bytes at 128 per chunk");
        for (_, packet) in h.timeline.iter().filter(|(_, p)| is_data_frame(p)) {
            assert_eq!(packet.wait_ms, Some(h.session.params.data_gap_ms));
            assert_eq!(packet.target, router_proto::BROADCAST);
        }

        // `begin` declared the length and CRC that `verify` was then checked against.
        let begins: Vec<Value> = h
            .timeline
            .iter()
            .filter_map(|(_, packet)| decode_envelope(&packet.bytes).ok())
            .filter_map(|envelope| match envelope.body {
                Value::Map(entries) => entries
                    .into_iter()
                    .find(|(k, _)| k.as_str() == Some("bl"))
                    .map(|(_, v)| v),
                _ => None,
            })
            .filter(|body| {
                matches!(body, Value::Map(fields)
                    if fields.iter().any(|(k, v)| k.as_str() == Some("q") && v.as_str() == Some("begin")))
            })
            .collect();
        assert_eq!(begins.len(), 2, "one begin per board");
        let Value::Map(fields) = &begins[0] else {
            panic!()
        };
        let field = |name: &str| {
            fields
                .iter()
                .find(|(k, _)| k.as_str() == Some(name))
                .and_then(|(_, v)| v.as_u64())
        };
        assert_eq!(field("len"), Some(h.session.image.len() as u64));
        assert_eq!(field("crc"), Some(u64::from(h.session.crc32)));
        assert_eq!(field("chunk"), Some(CHUNK as u64));
        assert_eq!(field("base"), Some(u64::from(layout::APP_BASE)));
    }

    /// `begin` is the one request that is retried blind, because it is idempotent and its
    /// reply is the slowest thing on the bus.
    #[test]
    fn a_lost_begin_is_retried_once_and_then_given_up_on() {
        let firmware = image_for(layout::APP_BASE, 512);
        let mut h = Harness::new(&firmware, params(vec![1, 2]));
        let mut begins_seen = 0usize;
        let progress = h.run(200, |h| {
            answer_v6(h, 1, layout::APP_BASE, None);
            // Board 2 answers `status` and then never speaks again.
            if let Some((verb, seq)) = h.outstanding(2) {
                match verb.as_str() {
                    "status" => {
                        let fields = status_fields(2, layout::APP_BASE);
                        h.reply(2, seq, fields);
                    }
                    "begin" => begins_seen += 1,
                    _ => {}
                }
            }
        });
        assert!(progress.done && !progress.ok);
        assert_eq!(h.board(1).state, BoardState::Running);
        assert_eq!(h.board(2).state, BoardState::NoReply(Phase::Begin));

        let begins_to_2 = h
            .timeline
            .iter()
            .filter(|(_, packet)| packet.target == 2)
            .filter(|(_, packet)| {
                decode_envelope(&packet.bytes)
                    .ok()
                    .is_some_and(|envelope| body_verb(&envelope) == Some("begin".into()))
            })
            .count();
        assert_eq!(begins_to_2, 2, "sent once, retried once, then abandoned");
    }

    #[test]
    fn repair_sends_exactly_the_union_of_the_missing_chunks_with_chunk_zero_first() {
        let firmware = image_for(layout::APP_BASE, 1_024);
        let mut h = Harness::new(&firmware, params(vec![1, 2]));

        // Board 1 is missing chunks 0 and 5; board 2 is missing 3. Both are complete on
        // the second map.
        let mut mapped = std::collections::HashMap::new();
        let progress = h.run(40, |h| {
            for id in [1i8, 2i8] {
                let Some((verb, _)) = h.outstanding(id) else {
                    continue;
                };
                if verb == "map" {
                    let round = mapped.entry(id).or_insert(0usize);
                    let bitmap = if *round == 0 {
                        Some(match id {
                            1 => vec![0b1101_1110],
                            _ => vec![0b1111_0111],
                        })
                    } else {
                        None
                    };
                    *round += 1;
                    answer_v6(h, id, layout::APP_BASE, bitmap);
                } else {
                    answer_v6(h, id, layout::APP_BASE, None);
                }
            }
        });
        assert!(progress.done && progress.ok, "{}", progress.detail);

        // The first eight data frames are the stream; what follows is the repair.
        let offsets = data_offsets(&h);
        assert_eq!(&offsets[..8], &[0, 128, 256, 384, 512, 640, 768, 896]);
        assert_eq!(
            &offsets[8..],
            &[0, 3 * 128, 5 * 128],
            "the union of both boards' gaps, chunk 0 first"
        );
    }

    #[test]
    fn a_verify_mismatch_marks_that_board_and_leaves_the_others_alone() {
        let firmware = image_for(layout::APP_BASE, 512);
        let mut h = Harness::new(&firmware, params(vec![1, 2]));
        let progress = h.run(40, |h| {
            answer_v6(h, 1, layout::APP_BASE, None);
            match h.outstanding(2) {
                Some((verb, seq)) if verb == "verify" => {
                    let len = h.session.image.len() as u32;
                    h.reply(
                        2,
                        seq,
                        vec![
                            (key("q"), Value::from("verify")),
                            (key("ok"), Value::from(false)),
                            (key("crc"), Value::from(0xDEAD_BEEFu32)),
                            (key("len"), Value::from(len)),
                        ],
                    );
                }
                _ => answer_v6(h, 2, layout::APP_BASE, None),
            }
        });
        assert!(progress.done && !progress.ok);
        assert_eq!(h.board(1).state, BoardState::Running);
        assert_eq!(h.board(2).state, BoardState::VerifyFailed);
        // A board that failed verification is never asked to run.
        let runs_to_2 = h
            .timeline
            .iter()
            .filter(|(_, packet)| packet.target == 2)
            .filter(|(_, packet)| {
                decode_envelope(&packet.bytes)
                    .ok()
                    .is_some_and(|envelope| body_verb(&envelope) == Some("run".into()))
            })
            .count();
        assert_eq!(runs_to_2, 0);
    }

    /// A lost `map` reply must not condemn a board that has the whole image. The bitmap is
    /// an optimisation -- it says which chunks to resend -- and `verify` is the verdict.
    #[test]
    fn a_board_whose_bitmap_never_arrives_is_still_verified() {
        let firmware = image_for(layout::APP_BASE, 512);
        let mut h = Harness::new(&firmware, params(vec![1, 2]));
        let progress = h.run(40, |h| {
            answer_v6(h, 1, layout::APP_BASE, None);
            // Board 2 answers everything except `map`.
            if h.outstanding(2).is_some_and(|(verb, _)| verb != "map") {
                answer_v6(h, 2, layout::APP_BASE, None);
            }
        });
        assert!(progress.done && progress.ok, "{}", progress.detail);
        assert_eq!(h.board(2).state, BoardState::Running);
    }

    /// A board that reports the right CRC under an `ok: false` is still a failure: the two
    /// disagree, and this host does not get to decide which half to believe.
    #[test]
    fn run_is_skipped_entirely_when_it_was_not_asked_for() {
        let firmware = image_for(layout::APP_BASE, 512);
        let mut h = Harness::new(
            &firmware,
            FwSessionParams {
                run_after: false,
                ..params(vec![1])
            },
        );
        let progress = h.run(40, |h| answer_v6(h, 1, layout::APP_BASE, None));
        assert!(progress.done && progress.ok);
        assert!(matches!(
            h.board(1).state,
            BoardState::Verified { crc32 } if crc32 == h.session.crc32
        ));
        assert!(h
            .timeline
            .iter()
            .all(|(_, packet)| decode_envelope(&packet.bytes)
                .ok()
                .is_none_or(|envelope| body_verb(&envelope) != Some("run".into()))));
    }

    // ------------------------------------------------------------ mixed fleet

    #[test]
    fn one_silent_board_puts_the_whole_fleet_on_the_blind_path() {
        let firmware = legacy_image(512);
        let mut h = Harness::new(&firmware, params(vec![1, 2]));
        let progress = h.run(50, |h| answer_v6(h, 1, layout::APP_BASE_LEGACY, None));

        assert!(progress.done && progress.ok, "{}", progress.detail);
        assert_eq!(h.board(1).kind, BoardKind::V6);
        assert_eq!(
            h.board(1).state,
            BoardState::LegacyBlind,
            "a v6 board on the blind path is not confirmed by anything"
        );
        assert_eq!(h.board(2).kind, BoardKind::Legacy);
        assert_eq!(h.board(2).state, BoardState::LegacyBlind);

        // The blind path's own frames went out: erase, then the image, then run.
        let words: Vec<String> = h
            .timeline
            .iter()
            .filter_map(|(_, packet)| decode_envelope(&packet.bytes).ok())
            .filter_map(|envelope| envelope.body.as_str().map(str::to_string))
            .collect();
        assert!(words.iter().any(|word| word == "ER"));
        assert_eq!(words.last().map(String::as_str), Some("RU"));
        assert!(!data_offsets(&h).is_empty());
    }

    // ------------------------------------------------------------- transport

    #[test]
    fn every_packet_is_unacked_and_uncollateable() {
        let firmware = image_for(layout::APP_BASE, 1_024);
        let mut h = Harness::new(&firmware, params(vec![1, 2]));
        h.run(40, |h| {
            answer_v6(h, 1, layout::APP_BASE, None);
            answer_v6(h, 2, layout::APP_BASE, None);
        });
        // 70 announce words, then discovery, begin, eight data frames, map, verify, run.
        assert!(h.bus.len() > 80, "only {} packets", h.bus.len());
        for packet in h.bus.sent() {
            assert!(!packet.needs_ack, "the worker would eat the reply as an ACK");
            assert!(!packet.collateable, "the outbox would drop all but the last");
            assert!(packet.address.is_empty(), "a non-empty address collates");
        }
    }

    #[test]
    fn packets_sent_counts_what_actually_left() {
        let firmware = image_for(layout::APP_BASE, 512);
        let mut h = Harness::new(&firmware, params(vec![1]));
        let progress = h.run(40, |h| answer_v6(h, 1, layout::APP_BASE, None));
        assert_eq!(progress.packets_sent, progress.packets_queued);
        assert_eq!(progress.packets_sent, h.bus.len());
        assert_eq!(progress.fraction, 1.0);
    }

    #[test]
    fn abort_clears_the_outbox_and_stops() {
        let firmware = image_for(layout::APP_BASE, 512);
        let mut h = Harness::new(&firmware, params(vec![1]));
        h.tick();
        h.session.abort(&h.bus);
        let progress = h.tick();
        assert!(progress.done && !progress.ok);
        assert_eq!(progress.detail, "aborted");
        assert_eq!(h.bus.clears(), 1);
    }

    // ----------------------------------------------------------- construction

    #[test]
    fn an_image_that_cannot_be_placed_is_refused_at_construction() {
        // Linked at 0x08000000: the `no_bootloader` build, which would program and verify
        // cleanly into an application slot and never start.
        let mut bad = vec![0u8; 0x400];
        bad[..4].copy_from_slice(&layout::RAM_END.to_le_bytes());
        bad[4..8].copy_from_slice(&0x0800_0241u32.to_le_bytes());
        assert!(matches!(
            FwSession::new(&bad, params(vec![1])),
            Err(FwSessionError::Image(_))
        ));

        assert!(matches!(
            FwSession::new(&image_for(layout::APP_BASE, 512), params(vec![])),
            Err(FwSessionError::NoTargets)
        ));

        for chunk in [0, 7, layout::BL_CHUNK_MAX + 8] {
            assert!(matches!(
                FwSession::new(
                    &image_for(layout::APP_BASE, 512),
                    FwSessionParams {
                        chunk_bytes: chunk,
                        ..params(vec![1])
                    }
                ),
                Err(FwSessionError::BadChunkSize { .. })
            ));
        }

        // An image that fits the v6 bank but not the legacy one, aimed at the legacy one.
        let oversize = legacy_image(layout::app_bank_bytes(layout::APP_BASE_LEGACY) + 8);
        assert!(matches!(
            FwSession::new(&oversize, params(vec![1])),
            Err(FwSessionError::Upload(_))
        ));
    }

    #[test]
    fn the_declared_crc_is_over_the_padded_image_the_boards_receive() {
        let firmware = image_for(layout::APP_BASE, 500);
        let session = FwSession::new(&firmware, params(vec![1])).unwrap();
        assert!(session.image_len().is_multiple_of(layout::FLASH_GRANULE));
        assert_eq!(session.image_crc32(), crc32c(&session.image));
        assert_eq!(session.chunk_count(), session.image_len().div_ceil(CHUNK));
        assert_eq!(session.image_base(), (layout::APP_BASE, BaseSource::Descriptor));
    }

    // ------------------------------------------------------------- bitmap maths

    #[test]
    fn a_bitmap_at_a_coarser_granularity_widens_to_every_chunk_it_covers() {
        // Same granularity: a straight translation.
        assert_eq!(missing_indices(&[0b1111_0101], 128, 128, 1_024), vec![1, 3]);
        // The board answered at 256 bytes while the session streams 128: each missing
        // reported chunk is two of ours.
        assert_eq!(missing_indices(&[0b0000_1101], 256, 128, 1_024), vec![2, 3]);
        // A short bitmap means "missing", never "assume present".
        assert_eq!(missing_indices(&[], 128, 128, 256), vec![0, 1]);
        assert_eq!(missing_indices(&[0xFF], 128, 128, 512), Vec::<usize>::new());
    }

    fn body_verb(envelope: &Envelope) -> Option<String> {
        let Value::Map(entries) = &envelope.body else {
            return None;
        };
        let (_, inner) = entries.iter().find(|(k, _)| k.as_str() == Some("bl"))?;
        let Value::Map(fields) = inner else {
            return None;
        };
        fields
            .iter()
            .find(|(k, _)| k.as_str() == Some("q"))?
            .1
            .as_str()
            .map(str::to_string)
    }
}
