// Port from librespot player.rs
use std::process::exit;
use std::future::Future;
use std::io::{self, Read, Seek, SeekFrom};
use std::pin::Pin;
use std::mem;
use std::task::{Context, Poll};
use std::sync::{Mutex,Arc};

use futures_util::stream::futures_unordered::FuturesUnordered;
use futures_util::{future, StreamExt, TryFutureExt};
use tokio::sync::{mpsc, oneshot};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::player::*;
use crate::server::{ServerMessage};
use crate::zone::{SpotifyJSEvent, RoonNowPlaying, RoonMessage};

use librespot::playback::player::{PlayerEvent};
use librespot::audio::{AudioFile, AudioDecrypt};
use librespot::playback::config::{Bitrate, PlayerConfig};
use librespot::core::session::Session;
use librespot::core::spotify_id::SpotifyId;
use librespot::metadata::{AudioItem, FileFormat};

const PRELOAD_NEXT_TRACK_BEFORE_END_DURATION_MS: u32 = 30000;

pub struct RoonPlayerLoadedTrack {
    audio_file:        Subfile<AudioDecrypt<AudioFile>>,
    audio:             AudioItem,
    start_position_ms: u32 ,
}

struct PlayerTrackLoader {
    session: Session,
    config: PlayerConfig,
}

impl PlayerTrackLoader {
    async fn find_available_alternative(&self, audio: AudioItem) -> Option<AudioItem> {
        if audio.available {
            Some(audio)
        } else if let Some(alternatives) = &audio.alternatives {
            let alternatives: FuturesUnordered<_> = alternatives
                .iter()
                .map(|alt_id| AudioItem::get_audio_item(&self.session, *alt_id))
                .collect();

            alternatives
                .filter_map(|x| future::ready(x.ok()))
                .filter(|x| future::ready(x.available))
                .next()
                .await
        } else {
            None
        }
    }

    fn stream_data_rate(&self, format: FileFormat) -> usize {
        match format {
            FileFormat::OGG_VORBIS_96 => 12 * 1024,
            FileFormat::OGG_VORBIS_160 => 20 * 1024,
            FileFormat::OGG_VORBIS_320 => 40 * 1024,
            FileFormat::MP3_256 => 32 * 1024,
            FileFormat::MP3_320 => 40 * 1024,
            FileFormat::MP3_160 => 20 * 1024,
            FileFormat::MP3_96 => 12 * 1024,
            FileFormat::MP3_160_ENC => 20 * 1024,
            FileFormat::MP4_128_DUAL => 16 * 1024,
            FileFormat::OTHER3 => 40 * 1024, // better some high guess than nothing
            FileFormat::AAC_160 => 20 * 1024,
            FileFormat::AAC_320 => 40 * 1024,
            FileFormat::MP4_128 => 16 * 1024,
            FileFormat::OTHER5 => 40 * 1024, // better some high guess than nothing
        }
    }

    async fn load_track(
        &self,
        spotify_id: SpotifyId,
        position_ms: u32,
    ) -> Option<RoonPlayerLoadedTrack> {
        let audio = match AudioItem::get_audio_item(&self.session, spotify_id).await {
            Ok(audio) => match self.find_available_alternative(audio).await {
                Some(audio) => audio,
                None => {
                    warn!(
                        "<{}> is not available",
                        spotify_id.to_uri().unwrap_or_default()
                    );
                    return None;
                }
            },
            Err(e) => {
                error!("Unable to load audio item: {:?}", e);
                return None;
            }
        };

        info!("Loading <{}> with Spotify URI <{}>", audio.name, audio.uri);

        if audio.duration < 0 {
            error!(
                "Track duration for <{}> cannot be {}",
                spotify_id.to_uri().unwrap_or_default(),
                audio.duration
            );
            return None;
        }

        //let duration_ms = audio.duration as u32;

        // (Most) podcasts seem to support only 96 bit Vorbis, so fall back to it
        let formats = match self.config.bitrate {
            Bitrate::Bitrate96 => [
                FileFormat::OGG_VORBIS_96,
                FileFormat::OGG_VORBIS_160,
                FileFormat::OGG_VORBIS_320,
            ],
            Bitrate::Bitrate160 => [
                FileFormat::OGG_VORBIS_160,
                FileFormat::OGG_VORBIS_96,
                FileFormat::OGG_VORBIS_320,
            ],
            Bitrate::Bitrate320 => [
                FileFormat::OGG_VORBIS_320,
                FileFormat::OGG_VORBIS_160,
                FileFormat::OGG_VORBIS_96,
            ],
        };

        let (format, file_id) =
            match formats
                .iter()
                .find_map(|format| match audio.files.get(format) {
                    Some(&file_id) => Some((*format, file_id)),
                    _ => None,
                }) {
                Some(t) => t,
                None => {
                    warn!("<{}> is not available in any supported format", audio.name);
                    return None;
                }
            };

        let bytes_per_second = self.stream_data_rate(format);
        let play_from_beginning = position_ms == 0;

        // This is only a loop to be able to reload the file if an error occurred
        // while opening a cached file.
        loop {
            let encrypted_file = AudioFile::open(
                &self.session,
                file_id,
                bytes_per_second,
                play_from_beginning,
            );


            let encrypted_file = match encrypted_file.await {
                Ok(encrypted_file) => encrypted_file,
                Err(e) => {
                    error!("Unable to load encrypted file: {:?}", e);
                    return None;
                }
            };
            //let is_cached = encrypted_file.is_cached();

            let stream_loader_controller = encrypted_file.get_stream_loader_controller();

            if play_from_beginning {
                // No need to seek -> we stream from the beginning
                stream_loader_controller.set_stream_mode();
            } else {
                // we need to seek -> we set stream mode after the initial seek.
                stream_loader_controller.set_random_access_mode();
            }

            let key = match self.session.audio_key().request(spotify_id, file_id).await {
                Ok(key) => key,
                Err(e) => {
                    error!("Unable to load decryption key: {:?}", e);
                    return None;
                }
            };

            let decrypted_file = AudioDecrypt::new(key, encrypted_file);
            let audio_file = Subfile::new(decrypted_file, 0xa7);
            return Some(RoonPlayerLoadedTrack {
                audio_file, // File handle
                audio,      // Track metadata
                start_position_ms: position_ms
            });
        }
    }
}

pub struct PlayerInternal {
    pub session: Session,
    pub config: PlayerConfig,
    pub commands: mpsc::UnboundedReceiver<PlayerCommand>,

    pub state: PlayerState,
    pub preload: PlayerPreload,
    pub event_senders: Vec<mpsc::UnboundedSender<PlayerEvent>>,

    pub auto_normalise_as_album: bool,

    //  XXX Roon
    pub player_server_rx: Arc<Mutex<UnboundedReceiver<ServerMessage>>>,
    pub player_roon_rx: Arc<Mutex<UnboundedReceiver<RoonMessage>>>,
    pub js_tx: Arc<Mutex<UnboundedSender<SpotifyJSEvent>>>,
    pub zone_id: String,
    pub yet_to_play: bool
}

impl Future for PlayerInternal {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        // While this is written as a future, it still contains blocking code.
        // It must be run on its own thread.

        loop {
            let mut all_futures_completed_or_not_ready = 0;

            // Handle commands from server 
            let msg = match self.player_server_rx.lock().unwrap().poll_recv(cx) {
                Poll::Ready(Some(msg)) => {
                    all_futures_completed_or_not_ready += 1;
                    Some(msg)
                },
                _ => None
            };

            if let Some(msg) = msg {
                self.handle_server_message(msg);
            }

            // Handle commands from roon 
            let cmd = match self.player_roon_rx.lock().unwrap().poll_recv(cx) {
                Poll::Ready(Some(cmd)) => {
                    all_futures_completed_or_not_ready += 1;
                    Some(cmd)
                },
                _ => None
            };

            if let Some(cmd) = cmd {
                self.handle_roon_command(cmd);
            }

            // process commands that were sent to us from spirc
            let cmd = match self.commands.poll_recv(cx) {
                Poll::Ready(None) => return Poll::Ready(()), // client has disconnected - shut down.
                Poll::Ready(Some(cmd)) => {
                    all_futures_completed_or_not_ready += 1;
                    Some(cmd)
                }
                _ => None,
            };

            if let Some(cmd) = cmd {
                self.handle_player_command(cmd);
            }
            if let PlayerState::Loading {
                ref mut loader,
                track_id,
                start_playback,
                play_request_id,
                ..
            } = self.state
            {
                match loader.as_mut().poll(cx) {
                    Poll::Ready(Ok(loaded_track)) => {
                        if start_playback {
                            let zone_id = self.zone_id.clone();
                            self.yet_to_play = false;
                            self.send_to_roon(SpotifyJSEvent::Play {
                                zone_id,
                                now_playing_info: RoonNowPlaying::new(loaded_track.audio.clone()),
                                position_ms:      loaded_track.start_position_ms.clone()
                            });
                            self.send_event(PlayerEvent::Loading {
                                track_id,
                                play_request_id,
                                position_ms: loaded_track.start_position_ms.clone(),
                            });
                            self.state = PlayerState::Playing {
                                track_id,
                                play_request_id,
                                position_ms: loaded_track.start_position_ms.clone(),
                                duration_ms: loaded_track.audio.duration.clone() as u32,
                                track: loaded_track,
                                suggested_to_preload_next_track: false,
                            };
                        } else {
                            self.send_event(PlayerEvent::Paused {
                                track_id,
                                play_request_id,
                                position_ms: loaded_track.start_position_ms.clone(),
                                duration_ms: loaded_track.audio.duration.clone() as u32,
                            });
                            self.state = PlayerState::Paused {
                                track_id,
                                play_request_id,
                                position_ms: loaded_track.start_position_ms.clone(),
                                duration_ms: loaded_track.audio.duration.clone() as u32,
                                track: loaded_track,
                                suggested_to_preload_next_track: false,
                            }
                        }
                    }
                    Poll::Ready(Err(_e)) => {
                        self.state = PlayerState::Stopped
                    }
                    Poll::Pending => (),
                }
            }
            if let PlayerPreload::Loading {
                ref mut loader,
                track_id,
            } = self.preload
            {
                match loader.as_mut().poll(cx) {
                    Poll::Ready(Ok(loaded_track)) => {
                        // Preloaded track ready, tell roon to start preloading
                        let zone_id = self.zone_id.clone();
                        self.send_to_roon(SpotifyJSEvent::Preload {
                            zone_id,
                            now_playing_info: RoonNowPlaying::new(loaded_track.audio.clone()),
                        });
                        self.preload = PlayerPreload::Ready {
                            loaded_track: Box::new(loaded_track),
                            track_id
                        };
                    }
                    Poll::Ready(Err(_e)) => {
                        self.preload = PlayerPreload::None
                    }
                    Poll::Pending => (),
                }
            }
            // This is the old logic for when to start downloading next track from spotify
            // If it is time to load the next track, let spirc know and it will call preload on the
            // this player with the next track id
            if let PlayerState::Playing {
                track_id,
                play_request_id,
                duration_ms,
                position_ms,
                ref mut suggested_to_preload_next_track,
                ..
            }
            | PlayerState::Paused {
                track_id,
                play_request_id,
                duration_ms,
                position_ms,
                ref mut suggested_to_preload_next_track,
                ..
            } = self.state
            {
                let time_to_end = duration_ms - position_ms;
                // XXX look into range_to_end_available in original librespot player
                if (!*suggested_to_preload_next_track) &&
                    time_to_end < PRELOAD_NEXT_TRACK_BEFORE_END_DURATION_MS {
                    *suggested_to_preload_next_track = true;
                    self.send_event(PlayerEvent::TimeToPreloadNextTrack {
                        track_id,
                        play_request_id,
                    });
                }
            }

            // Kill loop once session ends
            if self.session.is_invalid() {
                return Poll::Ready(());
            }

            // Nothing interesting has happened yet, no need to loop
            // Note** player is not responsible for delivering the "next" packet
            // Roon will grab what it needs and start playing. Because of this
            // loop back around if any messages came in, and once roon is done
            // schedule this for later
            if all_futures_completed_or_not_ready == 0 {
                return Poll::Pending;
            }

        }
    }
}

impl PlayerInternal {
    fn send_to_roon(&self, evt: SpotifyJSEvent) {
        info!("Sending message to Roon {:?}", evt);
        self.js_tx.lock().unwrap().send(evt).unwrap();
    }

    fn send_event(&mut self, event: PlayerEvent) {
        info!("Sending PlayerEvent {:?}", event);
        self.event_senders
            .retain(|sender| sender.send(event.clone()).is_ok());
    }

    fn load_track(
        &self,
        spotify_id: SpotifyId,
        position_ms: u32,
    ) -> impl Future<Output = Result<RoonPlayerLoadedTrack, ()>> + Send + 'static {
        // This method creates a future that returns the loaded stream and associated info.
        // Ideally all work should be done using asynchronous code. However, seek() on the
        // audio stream is implemented in a blocking fashion. Thus, we can't turn it into future
        // easily. Instead we spawn a thread to do the work and return a one-shot channel as the
        // future to work with.
        //
        //
        info!("Inside PlayerInternal load_track");

        let loader = PlayerTrackLoader {
            session: self.session.clone(),
            config: self.config.clone(),
        };

        let (result_tx, result_rx) = oneshot::channel();

        std::thread::spawn(move || {
            let data = futures_executor::block_on(loader.load_track(spotify_id, position_ms));
            if let Some(data) = data {
                let _ = result_tx.send(data);
            }
        });

        result_rx.map_err(|_| ())
    }
}

impl Drop for PlayerInternal {
    fn drop(&mut self) {
        debug!("drop PlayerInternal[{}]", self.session.session_id());
    }
}

struct Subfile<T: Read + Seek> {
    stream: T,
    offset: u64,
}

impl Subfile<AudioDecrypt<AudioFile>> {
    pub fn len(&mut self) -> usize {
        return self.stream.size() - self.offset as usize;
    }
}

impl<T: Read + Seek> Subfile<T> {
    pub fn new(mut stream: T, offset: u64) -> Subfile<T> {
        if let Err(e) = stream.seek(SeekFrom::Start(offset)) {
            error!("Subfile new Error: {}", e);
        }
        Subfile { stream, offset }
    }
}

impl<T: Read + Seek> Read for Subfile<T> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.stream.read(buf)
    }
}

impl<T: Read + Seek> Seek for Subfile<T> {
    fn seek(&mut self, mut pos: SeekFrom) -> io::Result<u64> {
        pos = match pos {
            SeekFrom::Start(offset) => SeekFrom::Start(offset + self.offset),
            x => x,
        };

        let newpos = self.stream.seek(pos)?;

        Ok(newpos.saturating_sub(self.offset))
    }
}
mod handle_roon_message;
mod handle_player_command;
mod handle_server_message;
