//! Stub types replacing `codex_realtime_webrtc` when the real crate is disabled.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::AtomicU16;
use std::sync::mpsc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RealtimeWebrtcEvent {
    Connected,
    LocalAudioLevel(u16),
    Closed,
    Failed(String),
}

pub struct RealtimeWebrtcSessionHandle {
    local_audio_peak: Arc<AtomicU16>,
}

impl fmt::Debug for RealtimeWebrtcSessionHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RealtimeWebrtcSessionHandle")
            .finish_non_exhaustive()
    }
}

impl RealtimeWebrtcSessionHandle {
    pub fn apply_answer_sdp(&self, _answer_sdp: String) -> Result<(), String> {
        Err("realtime WebRTC is disabled".into())
    }

    pub fn close(&self) {}

    pub fn local_audio_peak(&self) -> Arc<AtomicU16> {
        self.local_audio_peak.clone()
    }
}

pub struct StartedRealtimeWebrtcSession {
    pub offer_sdp: String,
    pub handle: RealtimeWebrtcSessionHandle,
    pub events: mpsc::Receiver<RealtimeWebrtcEvent>,
}

pub struct RealtimeWebrtcSession;

impl RealtimeWebrtcSession {
    pub fn start() -> Result<StartedRealtimeWebrtcSession, String> {
        Err("realtime WebRTC is disabled".into())
    }
}
