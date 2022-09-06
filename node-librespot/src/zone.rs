use simplelog::*;
use sha1::{Digest, Sha1};
use tokio::sync::mpsc::{UnboundedSender};

use librespot::core::config::{ConnectConfig, DeviceType, SessionConfig};
use librespot::playback::config::{PlayerConfig};
use librespot::connect::spirc::Spirc;
use librespot::core::session::Session;
use librespot::playback::mixer::{self, MixerConfig};
use librespot::metadata::{AudioItem};

// Custom player
use futures_util::{future, FutureExt, StreamExt};
use std::time::Duration;
use std::pin::Pin;
use std::time::Instant;
use std::process::exit;
use crate::player::{Player};
use std::sync::{Arc, Mutex};


use serde::{Serialize, Deserialize};
use crate::server::{ServerMessage};


#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RoonNowPlaying {
    pub track_id:   String,
    pub name:       String,
    pub album_name: Option<String>,
    pub artists:    Option<Vec<String>>,
    pub covers:     Option<Vec<String>>,
    pub show_name:  Option<String>
}
impl RoonNowPlaying {
    pub fn new(audio: AudioItem) -> RoonNowPlaying {
        let mut album_name = None;
        if let Some(album) = audio.album.clone() {
            album_name = Some(album.name);
        }

        let mut artists = None;
        if let Some(artist_objects) = audio.artists.clone() {
            artists = Some(artist_objects.iter().map(|a| a.name.clone()).collect());
        }

        let mut show_name = None;
        if let Some(show) = audio.show.clone() {
            show_name= Some(show.name);
        }

        let mut covers = None;
        if let Some(_covers) = audio.covers.clone() {
            // XXX Make this an object to include sizes
            covers= Some(_covers.iter().map(|f| f.to_base16().unwrap()).collect());
        }

        RoonNowPlaying {
            track_id:  audio.id.to_uri().unwrap(),
            name:      audio.name.clone(),
            album_name,
            artists,
            show_name,
            covers
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum SpotifyJSEvent {
    Play {
        zone_id:          String,
        now_playing_info: RoonNowPlaying,
        position_ms:      u32
    },
    Unpause {
        zone_id: String,
    },
    Pause {
        zone_id: String,
    },
    Seek {
        zone_id: String,
        seek_position_ms: u32
    },
    Stop {
        zone_id:     String,
    },
    Preload {
        zone_id:     String,
        now_playing_info: RoonNowPlaying
    },
    Clear {
        zone_id: String,
        slots: Vec<String>
    },
    VolumeSet {
        zone_id: String,
        volume:  u16 // 64k value
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum RoonMessage {
    EnableZone {
        name: String,
        id:   String
    },
    DisableZone {
        id: String
    },
    Playing             { id: String },
    Paused              { id: String },
    Unpaused            { id: String },
    Seeked              { id: String },
    NextTrack           { id: String },
    PreviousTrack       { id: String },
    Stopped             { id: String },
    EndedNaturally      { id: String },
    OnToNext            { id: String },
    Error               { id: String },
    Volume {
        id: String ,
        volume: u16
    },
    Time {
        id:     String,
        seek_position_ms: u32
    },
}

fn device_id(name: &str) -> String {
    hex::encode(Sha1::digest(name.as_bytes()))
}

pub struct Zone {
    commands:       UnboundedSender<RoonMessage>,
    server_player_tx: UnboundedSender<ServerMessage>,
    roon_player_tx: UnboundedSender<RoonMessage>,
}

impl Zone {
    pub fn new(name: String, id: String, js_tx: UnboundedSender<SpotifyJSEvent>) -> Zone {
        let (commands_tx,      mut commands_rx)  = tokio::sync::mpsc::unbounded_channel();
        let (server_player_tx, player_server_rx) = tokio::sync::mpsc::unbounded_channel();
        let (roon_player_tx,   player_roon_rx)   = tokio::sync::mpsc::unbounded_channel();

        let player_roon_arc   = Arc::new(Mutex::new(player_roon_rx));
        let player_server_arc = Arc::new(Mutex::new(player_server_rx));
        let js_callback_tx    = Arc::new(Mutex::new(js_tx));

        tokio::spawn(async move {
            const RECONNECT_RATE_LIMIT_WINDOW: Duration = Duration::from_secs(600);
            const RECONNECT_RATE_LIMIT: usize = 5;

            let mut last_credentials = None;
            let mut spirc: Option<Spirc> = None;
            let mut spirc_task: Option<Pin<_>> = None;
            let mut auto_connect_times: Vec<Instant> = vec![];
            let mut discovery = None;
            let mut connecting: Pin<Box<dyn future::FusedFuture<Output = _> + Send>> = Box::pin(future::pending());

            let player_config  = PlayerConfig::default();
            let session_config = SessionConfig {
                user_agent: String::from("FOOBARBUZZ"),
                device_id: device_id(&id),
                proxy:     None,
                ap_port:   None
            };
            let connect_config = ConnectConfig {
                name:            name.clone(),
                device_type:     DeviceType::default(),
                initial_volume:  Some(50),
                has_volume_ctrl: true,
                autoplay:        false,
            };
            info!("Starting discovery: {},{}",connect_config.name.clone(),session_config.device_id.clone());
            match librespot::discovery::Discovery::builder(session_config.device_id.clone())
                .name(name.clone())
                .device_type(librespot::discovery::DeviceType::Computer)
                .launch()
            {
                Ok(d) => discovery = Some(d),
                Err(err) => warn!("Could not initialize disovery: {}.", err),
            }

            // Port from librespot main.rs
            loop {
                tokio::select! {
                    msg = commands_rx.recv() => {
                        match msg {
                            Some(msg) => match msg {
                                RoonMessage::DisableZone { .. } => {
                                    info!("Shutting down zone {}", name.clone());
                                    break
                                },
                                _ => ()
                            },
                            _ => break
                        }
                    },
                    credentials = async {
                        match discovery.as_mut() {
                            Some(d) => d.next().await,
                            _ => None
                        }
                    }, if discovery.is_some() => {
                        match credentials {
                            Some(credentials) => {
                                last_credentials = Some(credentials.clone());
                                auto_connect_times.clear();

                                if let Some(spirc) = spirc.take() {
                                    spirc.shutdown();
                                }
                                if let Some(spirc_task) = spirc_task.take() {
                                    // Continue shutdown in its own task
                                    tokio::spawn(spirc_task);
                                }

                                connecting = Box::pin(Session::connect(
                                        session_config.clone(),
                                        credentials,
                                        None,//Cache
                                        true,
                                        ).fuse());
                            },
                            None => {
                                error!("Discovery stopped unexpectedly");
                                exit(1);
                            }
                        }
                    },
                    session = &mut connecting, if !connecting.is_terminated() => match session {
                        Ok((session,_)) => {
                            let mixer_config = MixerConfig::default();
                            let mixer = mixer::find(None).unwrap_or_else(|| {
                                info!("CREATING MIXER FIALED");
                                exit(1);
                            })(mixer_config);
                            let (player, _event_channel) = Player::new(
                                player_config.clone(),
                                session.clone(),
                                player_server_arc.clone(),
                                player_roon_arc.clone(),
                                js_callback_tx.clone(),
                                id.clone()
                            );
                            let (spirc_, spirc_task_) = Spirc::new(connect_config.clone(), session, player, mixer);
                            spirc      = Some(spirc_);
                            spirc_task = Some(Box::pin(spirc_task_));
                        },
                        Err(e) => {
                            error!("Connection failed: {}", e);
                            exit(1);
                        }
                    },
                    _ = async {
                        if let Some(task) = spirc_task.as_mut() {
                            task.await;
                        }
                    }, if spirc_task.is_some() => {
                        spirc_task = None;

                        warn!("Spirc shut down unexpectedly");

                        let mut reconnect_exceeds_rate_limit = || {
                            auto_connect_times.retain(|&t| t.elapsed() < RECONNECT_RATE_LIMIT_WINDOW);
                            auto_connect_times.len() > RECONNECT_RATE_LIMIT
                        };

                        match last_credentials.clone() {
                            Some(credentials) if !reconnect_exceeds_rate_limit() => {
                                auto_connect_times.push(Instant::now());

                                connecting = Box::pin(Session::connect(
                                        SessionConfig::default(),
                                        credentials,
                                        None,//Cache
                                        true
                                        ).fuse());
                            },
                            _ => {
                                error!("Spirc shut down too often.  Not reconnecting automatically.");
                                exit(1);
                            },
                        }
                    },
                }
            }

            info!("EXITED SPIRC");
            if let Some(spirc) = spirc {
                spirc.shutdown();
            }
        });
        Zone {
            server_player_tx,
            roon_player_tx, 
            commands: commands_tx,
        }
    }
    pub fn send(&mut self, msg: RoonMessage) {
        self.commands.send(msg.clone()).unwrap(); // Handles disable zone
        self.roon_player_tx.send(msg).unwrap();   // Handles rest
    }

    pub fn send_server_message(&mut self, msg: ServerMessage) {
        self.server_player_tx.send(msg).unwrap();
    }
}
