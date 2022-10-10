use super::*;

impl PlayerInternal {
    pub fn handle_player_command(&mut self, cmd: PlayerCommand) {
        match cmd {
            PlayerCommand::Load {
                track_id,
                play_request_id,
                play,
                position_ms,
            } => self.handle_load(track_id, play_request_id, play, position_ms),
            PlayerCommand::Play                       => self.handle_play(),
            PlayerCommand::Pause                      => self.handle_pause(),
            PlayerCommand::Stop                       => self.handle_stop(),
            PlayerCommand::Preload { track_id }       => self.handle_preload(track_id),
            PlayerCommand::Seek(position_ms)          => self.handle_seek(position_ms),
            PlayerCommand::AddEventSender(sender)     => self.event_senders.push(sender),
            PlayerCommand::EmitVolumeSetEvent(volume) => self.handle_volume_set(volume),
            // XXX No idea
            PlayerCommand::SetAutoNormaliseAsAlbum(_setting) => {
                //self.auto_normalise_as_album = setting
            }
        }
    }

    fn handle_play(&mut self) {
        info!(">>>> SPIRC CALLED PLAY");
       // We were in paused state, and spotify told us to play
       // Update state and let roon know what to do
       if let PlayerState::Paused {
           track_id,
           play_request_id,
           position_ms,
           duration_ms,
           suggested_to_preload_next_track,
           ..
       } = self.state {
            let old_track = match mem::replace(&mut self.state, PlayerState::Invalid) {
                PlayerState::Paused { track, .. } => track,
                _ => {
                    error!("Not in paused state!");
                    exit(1);
                }
            };
            if self.yet_to_play {
                // Case 2: We loaded a track but haven't played yet
                self.send_to_roon(SpotifyJSEvent::Play {
                    zone_id:          self.zone_id.clone(),
                    now_playing_info: RoonNowPlaying::new(old_track.audio.clone()),
                    position_ms:      old_track.start_position_ms.clone()
                });
                self.yet_to_play = false;
                self.state = PlayerState::Playing {
                    track: old_track,
                    track_id,
                    play_request_id,
                    position_ms,
                    duration_ms,
                    suggested_to_preload_next_track,
                };
            } else {
                // We have already played before, so the roon session should be set
                // Just tell roon to unpause and set state to playing
                self.send_to_roon(SpotifyJSEvent::Unpause {
                    zone_id: self.zone_id.clone(),
                });
                self.state = PlayerState::Playing {
                    track: old_track,
                    track_id,
                    play_request_id,
                    position_ms,
                    duration_ms,
                    suggested_to_preload_next_track,
                };
            }
       } else if let PlayerState::Loading {
           ..
       } = self.state {
           warn!("Called handle_play while in loading state, ignoring");
       } else {
           error!("Called handle_play while not in paused state");
           exit(1);
       }
    }

    fn handle_pause(&mut self) {
        info!(">>>> SPIRC CALLED PAUSE");
       // We were playing a song and pause was called from spotify
       // Tell roon and set our state to paused
       if let PlayerState::Playing {
           track_id,
           play_request_id,
           position_ms,
           duration_ms,
           suggested_to_preload_next_track,
           ..
       } = self.state {
            let old_track = match mem::replace(&mut self.state, PlayerState::Invalid) {
                PlayerState::Playing { track, .. } => track,
                _ => {
                    info!("Something is really wrong!");
                    exit(1);
                }
            };
            self.send_to_roon(SpotifyJSEvent::Pause {
                zone_id: self.zone_id.clone(),
            });
            self.state = PlayerState::Paused {
                track: old_track,
                track_id,
                play_request_id,
                position_ms,
                duration_ms,
                suggested_to_preload_next_track,
            };
       } else {
           error!("Called handle_pause from state other than playing");
           exit(1);
       }
    }

    // Spotify told us that the user disconnected, let roon know and update our state;
    fn handle_stop(&mut self) {
        info!(">>>> SPIRC CALLED STOP");
        match self.state {
            PlayerState::Invalid => {
                error!("Called handle_stop from player state Invalid");
                exit(1);
            },
            _ => {
                self.send_to_roon(SpotifyJSEvent::Stop {
                    zone_id:          self.zone_id.clone(),
                });
                self.yet_to_play = true; 
                self.state = PlayerState::Stopped;
            }
        }
    }

    // Spotify told us to start loading the next track ahead of time (although we decide when to 
    // message them, which then loops back here)
    fn handle_preload(&mut self, track_id: SpotifyId) {
        info!(">>>> SPIRC CALLED PRELOAD");
        let mut preload_track = true;
        // check whether the track is already loaded somewhere or being loaded.
        if let PlayerPreload::Loading {
            track_id: currently_loading,
            ..
        }
        | PlayerPreload::Ready {
            track_id: currently_loading,
            ..
        } = self.preload {
            if currently_loading == track_id {
                // we're already preloading the requested track.
                preload_track = false;
            } else {
                // we're preloading something else - cancel it.  
                self.preload = PlayerPreload::None;
            }
        }
        if let PlayerState::Playing {
            ref mut track,
            ..
        }
        | PlayerState::Paused {
            ref mut track,
            ..
        } = self.state {
            if track.audio.id == track_id {
                // we already have the requested track loaded.
                preload_track = false;
            }
        }
        // schedule the preload of the current track if desired.
        if preload_track {
            let loader = self.load_track(track_id, 0);
            self.preload = PlayerPreload::Loading {
                track_id,
                loader: Box::pin(loader),
            }
        }
    }

    // Spotify told us to load a track
    // Tell them we started loading, and update state
    fn handle_load(
        &mut self,
        track_id: SpotifyId,
        play_request_id: u64,
        play: bool,
        position_ms: u32,
    ) {
        info!(">>>> SPIRC CALLED LOAD");

        if let PlayerState::Playing {
            ref mut track,
            ..
        } | PlayerState::Paused {
            ref mut track,
            ..
        } = self.state {
            if track_id == track.audio.id {
                warn!("Already got this song..do nothing?");
                return;
            }
        }

        //Check if the requested track has been preloaded already. If so use the preloaded data.
        if let PlayerPreload::Ready {
            track_id: loaded_track_id,
            ..
        } = self.preload
        {
            if track_id == loaded_track_id {
                let preload = std::mem::replace(&mut self.preload, PlayerPreload::None);
                if let PlayerPreload::Ready {
                    track_id,
                    loaded_track,
                } = preload {
                    // let position_pcm = Self::position_ms_to_pcm(position_ms);
                    // XXX Fix stream here with a seek;
                    // XXX Update state!
                    // XXX Roon should be the one to say on to next in this situation?
                    

                    info!("Requested track id {:?} was already loaded, setting state to playing", track_id);
                    self.send_to_roon(SpotifyJSEvent::Play {
                        zone_id:          self.zone_id.clone(),
                        now_playing_info: RoonNowPlaying::new(loaded_track.audio.clone()),
                        position_ms:      loaded_track.start_position_ms.clone()
                    });
                    self.state = PlayerState::Playing {
                        track_id,
                        play_request_id,
                        position_ms: loaded_track.start_position_ms.clone(),
                        duration_ms: loaded_track.audio.duration.clone() as u32,
                        track:       *loaded_track,
                        suggested_to_preload_next_track: false,
                    };
                    return;
                } else {
                    error!("PlayerInternal handle_command_load: Invalid PlayerState");
                    exit(1);
                }
            } else {
                info!("Requested track id {:?} does not equal loaded_track_id {:?}, setting up loader", track_id, loaded_track_id);
            }
        }

        // Always tell spotify we are loading
        self.send_event(PlayerEvent::Loading {
            track_id,
            play_request_id,
            position_ms,
        });

        // Try to extract a pending loader from the preloading mechanism
        let loader = if let PlayerPreload::Loading { track_id: loaded_track_id, ..  } = self.preload {
            if (track_id == loaded_track_id) && (position_ms == 0) {
                let mut preload = PlayerPreload::None;
                std::mem::swap(&mut preload, &mut self.preload);
                if let PlayerPreload::Loading {
                    loader,
                    ..
                } = preload {
                    Some(loader)
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        self.preload = PlayerPreload::None;

        // If we don't have a loader yet, create one from scratch.
        let loader = loader.unwrap_or_else(|| Box::pin(self.load_track(track_id, position_ms)));
        //let loader = Box::pin(self.load_track(track_id, position_ms));

        let mut prev_track_id = None; 
        if let PlayerState::Playing { track_id, .. } |
               PlayerState::Paused  { track_id, .. }
        = self.state {
            prev_track_id = Some(track_id);
        }
        // Set ourselves to a loading state.
        self.state = PlayerState::Loading {
            track_id,
            play_request_id,
            start_playback: play,
            loader,
            prev_track_id,
        };
    }

    // Spotify told us to seek, let roon know
    fn handle_seek(&mut self, position_ms: u32) {
        info!(">>>> SPIRC CALLED SEEK");
       if let PlayerState::Playing {
           ..
       } | PlayerState::Paused {
           ..
       } = self.state {
            self.send_to_roon(SpotifyJSEvent::Seek {
                zone_id: self.zone_id.clone(),
                seek_position_ms: position_ms
            });
       } else {
           error!("Called handle_seek from neither Playing or Paused state");
           exit(1);
       }
    }

    fn handle_volume_set(&mut self, volume: u16) {
       info!(">>>>>>>> SPIRC TOLD US TO SET VOLUME");
       if let PlayerState::Playing {
           ..
       } | PlayerState::Paused {
           ..
       } = self.state {
            self.send_to_roon(SpotifyJSEvent::VolumeSet {
                zone_id: self.zone_id.clone(),
                volume
            });
       } else {
           warn!("Called handle_volume_set from neither Playing or Paused state, ignoring");
       }
    }
}
