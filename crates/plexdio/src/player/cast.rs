//! A cast session: playback running on one or more other Plex players. Commands go to every
//! player in the session (multi-cast); the first one ("leader") is polled for its timeline,
//! which the now-playing bar mirrors.

use crate::plex::remote::{PlayerInfo, RemoteTimeline};
use crate::plex::server::PmsClient;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub enum CastCmd {
    /// Play `ids` starting at track `start`, `offset_ms` into it.
    Play { ids: Vec<i64>, start: i64, offset_ms: u64 },
    Toggle,
    Next,
    Prev,
    Seek(u64),
    /// 0..=100.
    Volume(u32),
    Stop,
}

#[derive(Debug, Clone, Default)]
pub struct CastStatus {
    /// Last timeline of the leader.
    pub timeline: Option<RemoteTimeline>,
    /// The queue handed to the players.
    pub ids: Vec<i64>,
    /// Last problem talking to a player.
    pub error: Option<String>,
}

pub struct CastSession {
    pub players: Vec<PlayerInfo>,
    tx: Sender<CastCmd>,
    status: Arc<Mutex<CastStatus>>,
}

/// How often the leader's timeline is read.
const POLL: Duration = Duration::from_millis(1000);

impl CastSession {
    /// Start a session on `players` (the first is the leader). `wake` is called when the status
    /// changes, so the UI can redraw.
    pub fn start(pms: PmsClient, players: Vec<PlayerInfo>, wake: impl Fn() + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel::<CastCmd>();
        let status = Arc::new(Mutex::new(CastStatus::default()));
        let st = status.clone();
        let targets = players.clone();
        let spawned = std::thread::Builder::new().name("cast".into()).spawn(move || {
            let mut cmd_id = 0u64;
            let mut next_id = || {
                cmd_id += 1;
                cmd_id
            };
            loop {
                let cmd = match rx.recv_timeout(POLL) {
                    Ok(c) => Some(c),
                    Err(mpsc::RecvTimeoutError::Timeout) => None,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                };
                let mut error = None;
                if let Some(cmd) = cmd {
                    let leader_state = st.lock().ok().and_then(|s| s.timeline.as_ref().map(|t| t.state.clone())).unwrap_or_default();
                    let stop = cmd == CastCmd::Stop;
                    if let CastCmd::Play { ids, .. } = &cmd {
                        if let Ok(mut s) = st.lock() {
                            s.ids = ids.clone();
                        }
                    }
                    let queue = match &cmd {
                        CastCmd::Play { ids, .. } => match pms.create_play_queue(ids) {
                            Ok(q) => Some(q),
                            Err(e) => {
                                error = Some(format!("couldn't create a play queue: {}", e.user_message()));
                                None
                            }
                        },
                        _ => None,
                    };
                    for p in &targets {
                        let id = next_id();
                        let res = match &cmd {
                            CastCmd::Play { start, offset_ms, .. } => match queue {
                                Some(q) => pms.play_media(&p.id, q, *start, *offset_ms, id),
                                None => continue,
                            },
                            CastCmd::Toggle if leader_state == "playing" => pms.player_command(&p.id, "pause", &[], id),
                            CastCmd::Toggle => pms.player_command(&p.id, "play", &[], id),
                            CastCmd::Next => pms.player_command(&p.id, "skipNext", &[], id),
                            CastCmd::Prev => pms.player_command(&p.id, "skipPrevious", &[], id),
                            CastCmd::Seek(ms) => pms.player_command(&p.id, "seekTo", &[("offset", &ms.to_string())], id),
                            CastCmd::Volume(v) => pms.player_command(&p.id, "setParameters", &[("volume", &v.to_string())], id),
                            CastCmd::Stop => pms.player_command(&p.id, "stop", &[], id),
                        };
                        if let Err(e) = res {
                            log::warn!("cast command to {} failed: {e}", p.name);
                            error = Some(format!("{}: {}", p.name, e.user_message()));
                        }
                    }
                    if stop {
                        break;
                    }
                }
                let Some(leader) = targets.first() else { break };
                let tl = pms.player_timeline(&leader.id, next_id());
                if let Ok(mut s) = st.lock() {
                    match tl {
                        Ok(t) => s.timeline = Some(t),
                        Err(e) => error = error.or(Some(format!("{}: {}", leader.name, e.user_message()))),
                    }
                    if error.is_some() {
                        s.error = error;
                    }
                }
                wake();
            }
        });
        if spawned.is_err() {
            log::warn!("cannot start the cast thread");
        }
        Self { players, tx, status }
    }

    pub fn send(&self, cmd: CastCmd) {
        let _ = self.tx.send(cmd);
    }

    pub fn status(&self) -> CastStatus {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Names for the bar: "Kitchen" or "Kitchen + 2".
    pub fn label(&self) -> String {
        match self.players.as_slice() {
            [] => String::new(),
            [p] => p.name.clone(),
            [p, rest @ ..] => format!("{} + {}", p.name, rest.len()),
        }
    }
}
