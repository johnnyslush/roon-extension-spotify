use crate::zone::{RoonMessage};
use std::process::exit;
use std::mem;
use super::*;

impl PlayerInternal {

    pub fn handle_roon_command(&mut self, msg: RoonMessage) {
        match msg {
            RoonMessage::Playing {..}        => self.handle_roon_playing(),
            RoonMessage::Paused  {..}        => self.handle_roon_paused(),
            RoonMessage::Unpaused {..}       => self.handle_roon_unpaused(),
            RoonMessage::Time    {..}        => self.handle_roon_time(msg),
            RoonMessage::Seeked  {..}        => (),
            RoonMessage::NextTrack{..}       => self.handle_roon_next_track(),
            RoonMessage::PreviousTrack{..}   => self.handle_roon_previous_track(),
            RoonMessage::Stopped {..}        => self.handle_roon_stopped(),
            RoonMessage::EndedNaturally {..} => self.handle_roon_ended_naturally(),
            RoonMessage::OnToNext {..}       => self.handle_roon_on_to_next(),
            RoonMessage::Volume {..}         => self.handle_roon_volume(msg),
            RoonMessage::Error {..}          => (),
            _ => ()
        }
    }
    
    fn playing_to_paused(&mut self) {
        match mem::replace(&mut self.state, PlayerState::Invalid) {
            PlayerState::Playing { 
                track, 
                track_id,
                play_request_id,
                position_ms,
                duration_ms,
                suggested_to_preload_next_track,
            } => {
                self.state = PlayerState::Paused {
                    track,
                    track_id,
                    play_request_id,
                    position_ms,
                    duration_ms,
                    suggested_to_preload_next_track,
                };
            },
            _ => {
                error!("Called playing_to_pause from state other than playing");
                exit(1);
            }
        };
    }

    // Move from paused -> playing
    fn paused_to_playing(&mut self) {
        match mem::replace(&mut self.state, PlayerState::Invalid) {
            PlayerState::Paused { 
                track, 
                track_id,
                play_request_id,
                position_ms,
                duration_ms,
                suggested_to_preload_next_track,
            } => {
                self.state = PlayerState::Playing {
                    track,
                    track_id,
                    play_request_id,
                    position_ms,
                    duration_ms,
                    suggested_to_preload_next_track,
                };
            },
            _ => {
                error!("Called paused_to_playing from state other than Paused");
                exit(1);
            }
        };
    }


    fn handle_roon_playing(&mut self) {
        info!(">>>> GOT ROON PLAYING MESSAGE");
        // Already playing, no state change just tell spotify
        if let PlayerState::Playing {
            track_id,
            play_request_id,
            position_ms,
            duration_ms,
            ..
        } = self.state {
            self.send_event(PlayerEvent::Playing {
                track_id,
                play_request_id,
                position_ms,
                duration_ms,
            });

        // Player state was left in pause, update to playing and tell spotify
        } else if let PlayerState::Paused {
            track_id,
            play_request_id,
            position_ms,
            duration_ms,
            ..
        } = self.state {
            self.send_event(PlayerEvent::Playing {
                track_id,
                play_request_id,
                position_ms,
                duration_ms,
            });
            self.paused_to_playing();
        } else {
            error!("Got roon playing message while not in paused or playing state");
            exit(1);
        }
    }

    fn handle_roon_paused(&mut self) {
        info!(">>>> GOT ROON PAUSED MESSAGE");
        // Pause called from spotify somewhere
        // Extension told roon to pause
        // Roon confirmed pause
        if let PlayerState::Paused {
            track_id,
            play_request_id,
            position_ms,
            duration_ms,
            ..
        } = self.state {
            self.send_event(PlayerEvent::Paused {
                track_id,
                play_request_id,
                position_ms,
                duration_ms,
            });
        // Pause was called within roon
        // Tell spotify so that it reflects the pause inapp 
        // and update our state
        } else if let PlayerState::Playing {
            track_id,
            play_request_id,
            position_ms,
            duration_ms,
            ..
        } = self.state {
            self.send_event(PlayerEvent::Paused {
                track_id,
                play_request_id,
                position_ms,
                duration_ms,
            });
            self.playing_to_paused();
        } else {
            error!("Got roon paused message while not in playing or paused state");
            exit(1);
        }
    }

    fn handle_roon_unpaused(&mut self) {
        // Unpaused from spotify, roon confirmed, relay to spotify that it occurred
        info!(">>>> GOT ROON UNPAUSED MESSAGE");
        if let PlayerState::Playing {
            track_id,
            play_request_id,
            position_ms,
            duration_ms,
            ..
        } = self.state {
            self.send_event(PlayerEvent::Playing {
                track_id,
                play_request_id,
                position_ms,
                duration_ms,
            });

        // Roon called unpaused, relay to spotify and update our state
        } else if let PlayerState::Paused {
            track_id,
            play_request_id,
            position_ms,
            duration_ms,
            ..
        } = self.state {
            self.send_event(PlayerEvent::Playing {
                track_id,
                play_request_id,
                position_ms,
                duration_ms,
            });
            self.paused_to_playing();
        } else {
            error!("Got roon unpause message from state other than playing or paused");
            exit(1);
        }
    }

    fn handle_roon_time(&mut self, msg: RoonMessage) {
        // We are already playing, roon is just telling us where we
        // are at in the track. Update our state and relay to spotify
        info!(">>>> GOT ROON TIME MESSAGE");

        if let RoonMessage::Time { seek_position_ms, .. } = msg {
            
            if let PlayerState::Playing {
                track_id,
                play_request_id,
                ref mut position_ms,
                duration_ms,
                ..
            } = self.state {
                *position_ms = seek_position_ms;
                self.send_event(PlayerEvent::Playing {
                    track_id,
                    play_request_id,
                    position_ms: seek_position_ms,
                    duration_ms,
                });
            } else if let PlayerState::Paused {
                track_id,
                play_request_id,
                ref mut position_ms,
                duration_ms,
                ..
            } = self.state{
                *position_ms = seek_position_ms;
                self.send_event(PlayerEvent::Paused {
                    track_id,
                    play_request_id,
                    position_ms: seek_position_ms,
                    duration_ms,
                });
            } else {
                warn!("Got roon time message while not in playing/paused state");
            }
        }
    }

    fn handle_roon_stopped(&mut self) {
        info!(">>>>>>>>>>>> GOT ROON STOPPED");
        if let PlayerState::Playing {
            track_id,
            play_request_id,
            ..
        } | PlayerState::Paused {
            track_id,
            play_request_id,
            ..
        } = self.state {
            self.send_event(PlayerEvent::Stopped {
                track_id,
                play_request_id,
            })
        }
    }

    fn handle_roon_ended_naturally(&mut self) {
        info!(">>>>>>>>>>>> GOT ROON ENDED NATURALLY");
        if let PlayerState::Playing {
            track_id,
            play_request_id,
            ..
        } | PlayerState::Paused {
            track_id,
            play_request_id,
            ..
        } = self.state {
            self.send_event(PlayerEvent::Stopped {
                track_id,
                play_request_id,
            })
        }
    }

    fn handle_roon_on_to_next(&mut self) {
        // Need to send EndOfTrack to spirc, and it will turn around and call a load_track
        info!(">>>>>>>>>>>> GOT ROON ON TO NEXT");
        if let PlayerState::Playing {
            track_id,
            play_request_id,
            ..
        } = self.state {
            self.send_event(PlayerEvent::EndOfTrack {
                track_id,
                play_request_id,
            })
        } else {
            error!("Got roon on to next message while not in playing state");
            exit(1);
        }
    }

    fn handle_roon_next_track(&mut self) {
        info!(">>>>>>>>>>> GOT ROON NEXT TRACK");
        if let PlayerState::Playing {
            track_id,
            play_request_id,
            ..
        } |
        PlayerState::Paused {
            track_id,
            play_request_id,
            ..
        } = self.state {
            self.send_event(PlayerEvent::EndOfTrack {
                track_id,
                play_request_id,
            })
        } else {
            error!("Got roon next track message while not in playing or paused state");
            exit(1);
        }
    }
    fn handle_roon_previous_track(&mut self) {
        info!(">>>>>>>>>>> GOT ROON PREVIOUS TRACK");
        if let PlayerState::Playing {
            play_request_id,
            ..
        } |
        PlayerState::Paused {
            play_request_id,
            ..
        } = self.state {
            self.send_event(PlayerEvent::Prev {
                play_request_id,
            })
        } else {
            error!("Got roon prev track message while not in playing or paused state");
            exit(1);
        }
    }
    fn handle_roon_volume(&mut self, msg: RoonMessage) {
        info!(">>>>>>>>>>> GOT ROON CHANGE VOLUME");
        let volume = match msg {
            RoonMessage::Volume { volume, .. } => volume,
            _ => {
                error!("Got something other than volume message in roon volume handler");
                exit(1);
            }
        };
        self.send_event(PlayerEvent::VolumeSet {
            play_request_id: None,
            volume
        })
    }
}
