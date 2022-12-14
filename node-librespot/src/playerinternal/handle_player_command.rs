use super::*;

impl PlayerInternal {
    pub fn handle_player_command(&mut self, cmd: PlayerCommand) {
        info!("Got player command from spirc {:?}", cmd);
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
       // We were in paused state, and spotify told us to play
       // Update state and let roon know what to do
       if let PlayerState::Paused {
           track_id,
           play_request_id,
           position_ms,
           duration_ms,
           suggested_to_preload_next_track,
           preload_id,
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
                    preload_id: preload_id.clone(),
                    position_ms,
                    play_request_id,
                });
                self.yet_to_play = false;
                self.state = PlayerState::Playing {
                    track: old_track,
                    track_id,
                    play_request_id,
                    position_ms,
                    duration_ms,
                    suggested_to_preload_next_track,
                    preload_id,
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
                    preload_id,
                };
            }
       } else if let PlayerState::Loading {
           track_id,
           play_request_id,
           prev_track_id,
           preload_id,
           ..
       } = self.state {
           info!("Called handle_play while in loading state, setting start_playback = true");
            let loader = match mem::replace(&mut self.state, PlayerState::Invalid) {
                PlayerState::Loading { loader, .. } => loader,
                _ => {
                    error!("Not in loading state!");
                    exit(1);
                }
            };
           self.state = PlayerState::Loading {
               start_playback: true,
               track_id,
               play_request_id,
               loader,
               prev_track_id,
               preload_id,
           };
       } else {
           warn!("Called handle_play while not in paused or loading state");
       }
    }

    fn handle_pause(&mut self) {
       // We were playing a song and pause was called from spotify
       // Tell roon and set our state to paused
       if let PlayerState::Playing {
           track_id,
           play_request_id,
           position_ms,
           duration_ms,
           suggested_to_preload_next_track,
           preload_id,
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
                preload_id,
            };
       } else if let PlayerState::Loading {
           track_id,
           play_request_id,
           prev_track_id,
           preload_id,
           ..
       } = self.state {
           info!("Called handle_pause while in loading state, setting start_playback = false");
            let loader = match mem::replace(&mut self.state, PlayerState::Invalid) {
                PlayerState::Loading { loader, .. } => loader,
                _ => {
                    error!("Not in loading state!");
                    exit(1);
                }
            };
           self.state = PlayerState::Loading {
               start_playback: false,
               track_id,
               play_request_id,
               loader,
               prev_track_id,
               preload_id,
           };
       } else {
           warn!("Called handle_pause from state other than playing or loading");
       }
    }

    // Spotify told us that the user disconnected, let roon know and update our state;
    fn handle_stop(&mut self) {
        match self.state {
            PlayerState::Invalid => {
                warn!("Called handle_stop from player state Invalid");
            },
            PlayerState::Playing {
                play_request_id,
                track_id,
                ..
            } |
            PlayerState::Paused {
                play_request_id,
                track_id,
                ..
            } |
            PlayerState::Loading {
                play_request_id,
                track_id,
                ..
            } => {
                self.state = PlayerState::Stopped;
                self.yet_to_play = true;
                self.send_to_roon(SpotifyJSEvent::Stop {
                    zone_id:          self.zone_id.clone(),
                });
                self.send_event(PlayerEvent::Stopped {
                    play_request_id,
                    track_id
                });
            },
            _ => {
                self.yet_to_play = true; 
                self.state = PlayerState::Stopped;
                self.send_to_roon(SpotifyJSEvent::Stop {
                    zone_id:          self.zone_id.clone(),
                });
            }
        }
    }

    // Spotify told us to start loading the next track ahead of time (although we decide when to 
    // message them, which then loops back here)
    fn handle_preload(&mut self, track_id: SpotifyId) {
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

        // Cache should handle if preload is same as current track
        // also need to tell roon to queue current track anyways
        if preload_track {
            // schedule the preload of the current track if desired.
            let loader = self.load_track(track_id, 0);
            self.preload = PlayerPreload::Loading {
                track_id,
                loader: Box::pin(loader),
                preload_id: self.preload_id_generator.get()
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
                    preload_id,
                    ..
                } = preload {
                    // let position_pcm = Self::position_ms_to_pcm(position_ms);
                    // XXX Fix stream here with a seek;
                    // XXX Update state!
                    // XXX Roon should be the one to say on to next in this situation?
                    

                    info!("Requested track id {:?} was already loaded, setting state to playing", track_id);
                    self.send_to_roon(SpotifyJSEvent::Play {
                        zone_id:          self.zone_id.clone(),
                        now_playing_info: RoonNowPlaying::new(loaded_track.audio.clone()),
                        position_ms:      loaded_track.start_position_ms.clone(),
                        play_request_id,
                        preload_id:  Some(preload_id.clone())
                    });
                    self.state = PlayerState::Playing {
                        track_id,
                        play_request_id,
                        position_ms: loaded_track.start_position_ms.clone(),
                        duration_ms: loaded_track.audio.duration.clone() as u32,
                        track:       *loaded_track,
                        suggested_to_preload_next_track: false,
                        preload_id: Some(preload_id)
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
        let mut preload_id = None;
        let loader = if let PlayerPreload::Loading { track_id: loaded_track_id, ..  } = self.preload {
            if (track_id == loaded_track_id) && (position_ms == 0) {
                let mut preload = PlayerPreload::None;
                std::mem::swap(&mut preload, &mut self.preload);
                if let PlayerPreload::Loading {
                    loader,
                    preload_id: _preload_id,
                    ..
                } = preload {
                    preload_id = Some(_preload_id); // Need this for when play is called
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
            preload_id
        };
    }

    // Spotify told us to seek, let roon know
    fn handle_seek(&mut self, new_position_ms: u32) {
       if let PlayerState::Playing {
           ref mut position_ms,
           duration_ms,
           ..
       } | PlayerState::Paused {
           ref mut position_ms,
           duration_ms,
           ..
       } = self.state {
           if new_position_ms < duration_ms {
               *position_ms = new_position_ms.clone();
           }
            self.send_to_roon(SpotifyJSEvent::Seek {
                zone_id: self.zone_id.clone(),
                seek_position_ms: new_position_ms
            });
       } else {
           warn!("Called handle_seek from neither Playing or Paused state");
       }
    }

    fn handle_volume_set(&mut self, volume: u16) {
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
