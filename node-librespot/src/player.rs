// Port from librespot player.rs
use std::future::Future;
use std::pin::Pin;
use std::{thread};

use tokio::sync::{mpsc};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::playerinternal::*;
use crate::server::{ServerMessage};
use crate::zone::{SpotifyJSEvent,RoonMessage};

use librespot::playback::player::{PlayerEventChannel, PlayerEvent};
use librespot::connect::spirc::{PlayerImpl};
use librespot::playback::config::{PlayerConfig};
use librespot::core::session::Session;
use librespot::core::spotify_id::SpotifyId;
use librespot::core::util::SeqGenerator;
use std::sync::{Mutex,Arc};

pub struct Player {
    commands: Option<mpsc::UnboundedSender<PlayerCommand>>,
    thread_handle: Option<thread::JoinHandle<()>>,
    play_request_id_generator: SeqGenerator<u64>,
}


pub enum PlayerCommand {
    Load {
        track_id: SpotifyId,
        play_request_id: u64,
        play: bool,
        position_ms: u32,
    },
    Preload {
        track_id: SpotifyId,
    },
    Play,
    Pause,
    Stop,
    Seek(u32),
    AddEventSender(mpsc::UnboundedSender<PlayerEvent>),
    EmitVolumeSetEvent(u16),
    SetAutoNormaliseAsAlbum(bool),
}

impl Player {
    pub fn new(
        config: PlayerConfig,
        session: Session,
        player_server_rx: Arc<Mutex<UnboundedReceiver<ServerMessage>>>,
        player_roon_rx:   Arc<Mutex<UnboundedReceiver<RoonMessage>>>,
        js_tx: Arc<Mutex<UnboundedSender<SpotifyJSEvent>>>,
        zone_id: String
    ) -> (Player, PlayerEventChannel)
    {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (event_sender, event_receiver) = mpsc::unbounded_channel();

        let handle = thread::spawn(move || {
            let internal = PlayerInternal {
                session,
                config,
                commands: cmd_rx,
                preload_id_generator: SeqGenerator::new(0),
                state: PlayerState::Stopped,
                preload: PlayerPreload::None,
                event_senders: [event_sender].to_vec(),
                auto_normalise_as_album: false,
                player_server_rx,
                player_roon_rx,
                js_tx,
                zone_id,
                yet_to_play: true
            };

            // While PlayerInternal is written as a future, it still contains blocking code.
            // It must be run by using block_on() in a dedicated thread.
            futures_executor::block_on(internal);
        });

        (
            Player {
                commands: Some(cmd_tx),
                thread_handle: Some(handle),
                play_request_id_generator: SeqGenerator::new(0),
            },
            event_receiver,
        )
    }

    fn command(&self, cmd: PlayerCommand) {
        if let Some(commands) = self.commands.as_ref() {
            if let Err(e) = commands.send(cmd) {
                error!("Player Commands Error: {}", e);
            }
        }
    }
}

impl PlayerImpl for Player {

    fn load(&mut self, track_id: SpotifyId, start_playing: bool, position_ms: u32) -> u64 {
        let play_request_id = self.play_request_id_generator.get();
        self.command(PlayerCommand::Load {
            track_id,
            play_request_id,
            play: start_playing,
            position_ms,
        });

        play_request_id
    }

    fn preload(&self, track_id: SpotifyId) {
        self.command(PlayerCommand::Preload { track_id });
    }

    fn play(&self) {
        self.command(PlayerCommand::Play)
    }

    fn pause(&self) {
        self.command(PlayerCommand::Pause)
    }

    fn stop(&self) {
        self.command(PlayerCommand::Stop)
    }

    fn seek(&self, position_ms: u32) {
        self.command(PlayerCommand::Seek(position_ms));
    }

    fn get_player_event_channel(&self) -> PlayerEventChannel {
        let (event_sender, event_receiver) = mpsc::unbounded_channel();
        self.command(PlayerCommand::AddEventSender(event_sender));
        event_receiver
    }

    fn emit_volume_set_event(&self, volume: u16) {
        self.command(PlayerCommand::EmitVolumeSetEvent(volume));
    }

    fn set_auto_normalise_as_album(&self, setting: bool) {
        self.command(PlayerCommand::SetAutoNormaliseAsAlbum(setting));
    }
}

impl Drop for Player  {
    fn drop(&mut self) {
        self.commands = None;
        if let Some(handle) = self.thread_handle.take() {
            match handle.join() {
                Ok(_) => (),
                Err(e) => error!("Player thread Error: {:?}", e),
            }
        }
    }
}

pub enum PlayerPreload {
    None,
    Loading {
        track_id: SpotifyId,
        loader: Pin<Box<dyn Future<Output = Result<RoonPlayerLoadedTrack, ()>> + Send>>,
        preload_id: u64
    },
    Ready {
        track_id: SpotifyId,
        loaded_track: Box<RoonPlayerLoadedTrack>,
        preload_id: u64
    },
}

pub enum PlayerState {
    Invalid,
    Stopped,
    Loading {
        track_id: SpotifyId,
        play_request_id: u64,
        start_playback: bool,
        loader: Pin<Box<dyn Future<Output = Result<RoonPlayerLoadedTrack, ()>> + Send>>,
        prev_track_id: Option<SpotifyId>,
        preload_id: Option<u64>,
    },
    Playing {
        track_id: SpotifyId,
        play_request_id: u64,
        track: RoonPlayerLoadedTrack,
        position_ms: u32,
        duration_ms: u32,
        suggested_to_preload_next_track: bool,
        preload_id: Option<u64>,
    },
    Paused {
        track_id: SpotifyId,
        play_request_id: u64,
        track: RoonPlayerLoadedTrack,
        position_ms: u32,
        duration_ms: u32,
        suggested_to_preload_next_track: bool,
        preload_id: Option<u64>,
    },
}

impl ::std::fmt::Debug for PlayerCommand {
    fn fmt(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
        match *self {
            PlayerCommand::Load {
                track_id,
                play,
                position_ms,
                ..
            } => f
                .debug_tuple("Load")
                .field(&track_id)
                .field(&play)
                .field(&position_ms)
                .finish(),
            PlayerCommand::Preload { track_id } => {
                f.debug_tuple("Preload").field(&track_id).finish()
            }
            PlayerCommand::Play => f.debug_tuple("Play").finish(),
            PlayerCommand::Pause => f.debug_tuple("Pause").finish(),
            PlayerCommand::Stop => f.debug_tuple("Stop").finish(),
            PlayerCommand::Seek(position) => f.debug_tuple("Seek").field(&position).finish(),
            PlayerCommand::AddEventSender(_) => f.debug_tuple("AddEventSender").finish(),
            PlayerCommand::EmitVolumeSetEvent(volume) => {
                f.debug_tuple("VolumeSet").field(&volume).finish()
            }
            PlayerCommand::SetAutoNormaliseAsAlbum(setting) => f
                .debug_tuple("SetAutoNormaliseAsAlbum")
                .field(&setting)
                .finish(),
        }
    }
}

/*
impl ::std::fmt::Debug for PlayerState {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        use PlayerState::*;
        match *self {
            Stopped => f.debug_struct("Stopped").finish(),
            Loading {
                track_id,
                play_request_id,
                ..
            } => f
                .debug_struct("Loading")
                .field("track_id", &track_id)
                .field("play_request_id", &play_request_id)
                .finish(),
            Ready {
                track
            } => f.debug_struct("Ready").finish(),
        }
    }
}
*/
