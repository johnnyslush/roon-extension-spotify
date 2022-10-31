use tokio::sync::mpsc::unbounded_channel;
use tokio::sync::mpsc::{UnboundedReceiver};
use std::collections::HashMap;
use crate::server::{ServerMessage, ServerReply};
use crate::zone::*;

#[derive(Debug)]
pub enum HostMessage {
    Stop
}

pub async fn run<F: Fn(SpotifyJSEvent)>(
    mut rx:         UnboundedReceiver<RoonMessage>,
    mut server_rx:  UnboundedReceiver<ServerMessage>,
    mut host_rx:    UnboundedReceiver<HostMessage>,
    f: F

) -> std::io::Result<()> {
    let mut zones                = HashMap::<String, Zone>::new();
    let (zones_tx, mut zones_rx) = unbounded_channel();
    loop {
        tokio::select! {
            // Disable all zones
            hostmsg = host_rx.recv() => {
                match hostmsg {
                    Some(hostmsg) => {
                        match hostmsg {
                            HostMessage::Stop => {
                                info!("GOT HOST MESSAGE");
                                break
                            }
                        }
                    },
                    _ => break
                }
            },
            // Listen for messages to send to Roon
            zonemsg = zones_rx.recv() => {
                match zonemsg {
                    Some(zonemsg) => {
                        f(zonemsg);
                    },
                    _ => break
                }
            },
            // Listen for messages from Roon
            msg = rx.recv() => {
                match msg {
                    Some(msg) => {
                        let cpy = msg.clone();
                        match msg {
                            RoonMessage::EnableZone {
                                name,
                                id
                            } => {
                                if !zones.contains_key(&id) {
                                    let zone = Zone::new(name.clone(), id.clone(), zones_tx.clone());
                                    zones.insert(id, zone);
                                }
                            },
                            RoonMessage::DisableZone {
                                id
                            } => {
                                if let Some(zone) = zones.get_mut(&id) {
                                    info!("REMOVED ZONE {}",id);
                                    zone.send(cpy);
                                    zones.remove(&id);
                                }
                            },
                            RoonMessage::RenameZone          { id, .. } |
                            RoonMessage::Playing             { id, .. } |
                            RoonMessage::Paused              { id, .. } |
                            RoonMessage::Unpaused            { id, .. } |
                            RoonMessage::Time                { id, .. } |
                            RoonMessage::Seeked              { id, .. } |
                            RoonMessage::NextTrack           { id, .. } |
                            RoonMessage::PreviousTrack       { id, .. } |
                            RoonMessage::Stopped             { id, .. } |
                            RoonMessage::EndedNaturally      { id, .. } |
                            RoonMessage::OnToNext            { id, .. } |
                            RoonMessage::Volume              { id, .. } |
                            RoonMessage::Error               { id, .. } => {
                                if let Some(zone) = zones.get_mut(&id) {
                                    zone.send(cpy);
                                }
                            }
                        }
                    }
                    _ => break
                }
            },
            // Respond to HTTP Queries
            servermsg = server_rx.recv() => {
                match servermsg {
                    Some(servermsg) => match servermsg {
                        ServerMessage::TrackInfo {
                            zone_id,
                            track_id,
                            responder,
                        } => {
                            if let Some(zone) = zones.get_mut(&zone_id.clone()) {
                                zone.send_server_message(
                                    ServerMessage::TrackInfo {
                                        zone_id,
                                        track_id,
                                        responder
                                    }
                                );
                            } else {
                                info!("Bad zone requested {}", zone_id);
                                responder.send(ServerReply::NotFound).unwrap();
                            }
                        },
                        ServerMessage::TrackRead {
                            zone_id,
                            track_id,
                            start,
                            end,
                            out,
                            responder
                        } => {
                            if let Some(zone) = zones.get_mut(&zone_id.clone()) {
                                zone.send_server_message(
                                    ServerMessage::TrackRead {
                                        zone_id,
                                        track_id,
                                        start,
                                        end,
                                        out,
                                        responder
                                    }
                                );
                            } else {
                                responder.send(ServerReply::NotFound).unwrap();
                            }

                        }
                    },
                    _ => break
                }
            },
        }
    }
    info!("EXITED DEVICES THREAD");
    Ok(())
}
