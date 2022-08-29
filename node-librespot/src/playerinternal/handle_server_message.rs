use crate::server::{ServerMessage,ServerReply};
use super::*;

impl PlayerInternal {
    pub fn handle_server_message(&mut self, msg: ServerMessage) {
        match msg {
            ServerMessage::TrackInfo {
                track_id,
                responder,
                ..
            } => {
                match &mut self.state {
                    PlayerState::Playing { track, .. } |
                    PlayerState::Paused  { track, .. }
                    => {
                        if track_id == track.audio.id.clone().to_uri().unwrap() {
                            responder.send(ServerReply::TrackInfo { 
                                file_size: track.audio_file.len(),
                                track_id: track.audio.id.clone().to_uri().unwrap()
                            }).unwrap();
                            return;
                        }
                    },
                    _ => ()
                }

                match &mut self.preload {
                    PlayerPreload::Ready {
                        loaded_track,
                        ..
                    } => {
                        let track = &mut *loaded_track;
                        if track_id == track.audio.id.clone().to_uri().unwrap() {
                            responder.send(ServerReply::TrackInfo { 
                                file_size: track.audio_file.len(),
                                track_id: track.audio.id.clone().to_uri().unwrap()
                            }).unwrap();
                        } else {
                            // Bad track id, not in current state or preloaded state
                            responder.send(ServerReply::Busy).unwrap();
                        }
                    }
                    _ => {
                        // Preloader not ready
                        responder.send(ServerReply::Busy).unwrap();
                    }
                }
            },
            ServerMessage::TrackRead {
                track_id,
                start,
                out,
                responder,
                ..
            } => {
                match &mut self.state {
                    PlayerState::Playing { track, .. } |
                    PlayerState::Paused  { track, .. }
                    => {
                        if track_id == track.audio.id.clone().to_uri().unwrap() {
                            track.audio_file.seek(SeekFrom::Start(start as u64)).unwrap();
                            let file_size = track.audio_file.len();
                            let read_len  = track.audio_file.read(&mut out.lock().unwrap()).unwrap();
                            responder.send(ServerReply::TrackRead { 
                                read_len,
                                file_size,
                                track_id: track.audio.id.clone().to_uri().unwrap()
                            }).unwrap();
                            return;
                        }
                    },
                    _ => ()
                }

                match &mut self.preload {
                    PlayerPreload::Ready {
                        loaded_track,
                        ..
                    } => {
                        let track = &mut *loaded_track;
                        if track_id == track.audio.id.clone().to_uri().unwrap() {
                            track.audio_file.seek(SeekFrom::Start(start as u64)).unwrap();
                            let file_size = track.audio_file.len();
                            let read_len  = track.audio_file.read(&mut out.lock().unwrap()).unwrap();
                            responder.send(ServerReply::TrackRead { 
                                read_len,
                                file_size,
                                track_id: track.audio.id.clone().to_uri().unwrap()
                            }).unwrap();
                        } else {
                            // Bad track id, not in current state or preloaded state
                            responder.send(ServerReply::Busy).unwrap();
                        }
                    }
                    _ => {
                        // Preloader not ready
                        responder.send(ServerReply::Busy).unwrap();
                    }
                }
            }
        }
    }
}
