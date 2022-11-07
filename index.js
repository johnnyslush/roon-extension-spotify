const RoonApi           = require('node-roon-api');
const RoonApiSettings   = require('node-roon-api-settings');
const RoonApiTransport  = require('node-roon-api-transport');
const RoonApiStatus     = require('node-roon-api-status');
const RoonApiAudioInput = require('node-roon-api-audioinput');
const pino              = require('pino');
const pretty            = require('pino-pretty');
const path              = require('path');
const { Host }          = require('./node-librespot/index.js');

/////////
const log_dir = /node(.exe)?$/.test(process.argv[0]) ? process.cwd() : path.join(process.execPath, '..');
const transport = pino.transport({
    targets: [
        { target: 'pino-pretty' },
        { target: path.join(__dirname,'transport.js'), options: { destination: path.join(log_dir, '/roon-extension-spotify.log') }},
    ]
});
const logger = pino(transport);
const os     = require('os');

// Default to localhost, use local network ip if found
const get_ip = () => {
    let librespot_http_url = '127.0.0.1';
    const nets   = os.networkInterfaces();
    for (const name of Object.keys(nets)) {
        for (const net of nets[name]) {
            if (net.family === 'IPv4' && !net.internal) {
                librespot_http_url = net.address;
            }
        }
    }
    return librespot_http_url
}

let sessions  = {};
let zones     = {};
let zoneSlots = {};

let global_core;
let host;
let librespot_http_port;
let librespot_http_url;

async function handle_core_paired(core) {
    if (!host) {
        // Create new host
        librespot_http_url = get_ip();
        host = new Host({
            log_dir,
            base_url: librespot_http_url === "127.0.0.1" ? librespot_http_url : "0.0.0.0", // Host to listen on locally
            listen_port: null,
            callbacks: {
                Play:      spotify_tells_us_to_play,
                Pause:     spotify_tells_us_to_pause,
                Unpause:   spotify_tells_us_to_unpause,
                Seek:      spotify_tells_us_to_seek,
                Preload:   spotify_tells_us_to_preload,
                Clear:     spotify_tells_us_to_clear,
                VolumeSet: spotify_tells_us_to_set_volume,
                Stop:      spotify_tells_us_to_stop,
            }
        });
    }
    await host.start();
    librespot_http_port = host.port();
    logger.info(`Host started and listening at ${librespot_http_url}:${librespot_http_port}`);
    core.services.RoonApiTransport.subscribe_zones((response, msg) => {
        if (response == 'Subscribed') {
            msg.zones.forEach(e => { zones[e.zone_id] = e; });
            msg.zones.forEach(z => {
                host.send_roon_message({
                    type: 'EnableZone',
                    name: z.display_name,
                    id:   z.zone_id
                });
            })

        } else if (response == 'Changed') {
            if (msg.zones_removed) msg.zones_removed.forEach(e => delete(zones[e.zone_id]));
            if (msg.zones_added)   msg.zones_added  .forEach(e => zones[e.zone_id] = e );

            if(msg.zones_removed) {
                msg.zones_removed.forEach(id => {
                    host.send_roon_message({
                        type: 'DisableZone',
                        id
                    });
                })
            }
            if(msg.zones_added) {
                msg.zones_added.forEach(z => {
                    host.send_roon_message({
                        type: 'EnableZone',
                        name: z.display_name,
                        id:   z.zone_id,
                    });
                })
            }
            if (msg.zones_changed) {
                msg.zones_changed.forEach(z => {



                    let oldz = zones[z.zone_id];
                    if (oldz.display_name !== z.display_name) {
                        logger.info("Sending rename message from javascript");
                        host.send_roon_message({
                            type:   'RenameZone',
                            id:     z.zone_id,
                            name:   z.display_name
                        });
                    }
                    // Zone has volume
                    if (oldz && oldz.outputs.length == 1 && oldz.outputs[0].volume && oldz.outputs[0].volume.step) {
                        const volumeHandle = oldz.outputs[0].volume;
                        if (z && z.outputs.length == 1 && z.outputs[0].volume && z.outputs[0].volume.step) {
                            const newVolumeHandle = z.outputs[0].volume;
                            if (volumeHandle.is_muted != newVolumeHandle.is_muted) {
                                if (newVolumeHandle.is_muted) {
                                    host.send_roon_message({
                                        type:   'Volume',
                                        id:     z.zone_id,
                                        volume: 0
                                    });
                                } else {
                                    const newVolume = Math.ceil(((newVolumeHandle.value - newVolumeHandle.min) / (newVolumeHandle.max - newVolumeHandle.min)) * 65535);
                                    host.send_roon_message({
                                        type:   'Volume',
                                        id:     z.zone_id,
                                        volume: newVolume
                                    });
                                }
                            } else if (volumeHandle.value != newVolumeHandle.value) {
                                logger.info('CHANGING VOLUME');
                                const newVolume = Math.ceil(((newVolumeHandle.value - newVolumeHandle.min) / (newVolumeHandle.max - newVolumeHandle.min)) * 65535);
                                host.send_roon_message({
                                    type:   'Volume',
                                    id:     z.zone_id,
                                    volume: newVolume
                                });
                            }
                        } else {
                            logger.info('Grouped zone volume not supported');
                        }
                    } else {
                        logger.info('Grouped zone volume not supported');
                    }
                    zones[z.zone_id] = z;
                })
            }
        }
    });
    global_core = core;
    svc_status.set_status("Ready.", false);
}

async function handle_core_unpaired(core) {
    logger.info({msg: "UNPAIRED", core});
    logger.info('stopping host and deleting');
    await host.stop();
    logger.info('succesfully stopped host');
    sessions  = {};
    zones     = {};
    zoneSlots = {};
}

const roon = new RoonApi({
    extension_id:        'com.roon.extension.spotify',
    display_name:        'Roon Spotify Connect',
    display_version:     "1.0.0",
    publisher:           'johnnyslush',
    email:               'johnnyslush551@gmail.com',

    force_server:  true,
    core_paired:   handle_core_paired,
    core_unpaired: handle_core_unpaired,
})

const svc_status = new RoonApiStatus(roon);
roon.init_services({
        provided_services: [ /*svc_settings ,*/ svc_status ],
        required_services: [ RoonApiAudioInput, RoonApiTransport ],
});

roon.start_discovery();

async function getOrCreateSession(zone_id) {
    if (sessions[zone_id]) return sessions[zone_id];

    let p = new Promise((resolve, reject) => {
        let session = global_core.services.RoonApiAudioInput.begin_session({
                zone_id,
                display_name: "Spotify",
                icon_url: "https://developer.spotify.com/assets/branding-guidelines/icon3@2x.png"
            },
            (msg, body) => {
                logger.info({msg:"SESSION: ", message:msg, body});
                if (msg == "SessionBegan") {
                    sessions[zone_id] = body.session_id;
                    // Setup transport controls
                    global_core.services.RoonApiAudioInput.update_transport_controls({
                        session_id: body.session_id,
                        controls: {
                            is_previous_allowed: true,
                            is_next_allowed: true
                        }
                    });
                    // Tell spotify about volume set in roon
                    let z = zones[zone_id];
                    // Zone has volume
                    if (z && z.outputs.length == 1 && z.outputs[0].volume && z.outputs[0].volume.step) {
                        const volumeHandle = z.outputs[0].volume;
                        host.send_roon_message({
                            type:   'Volume',
                            id:     z.zone_id,
                            volume: Math.ceil((volumeHandle.value - volumeHandle.min) / (volumeHandle.max - volumeHandle.min) * 65535)
                        });
                    }

                    resolve(body.session_id);
                } else if (msg == "TransportControl") {
                    if (body.control == "next")
                        host.send_roon_message({
                            type:        'NextTrack',
                            id:          zone_id,
                        });
                    else if (body.control == "previous")
                        host.send_roon_message({
                            type:        'PreviousTrack',
                            id:          zone_id,
                        });

                } else if (msg == "ZoneNotFound") {
                    delete(sessions[zone_id]);
                    reject();

                } else if (msg == "ZoneLost") {
                    delete(sessions[zone_id]);
                    // XXX

                } else if (msg == "SessionEnded") {
                    delete(sessions[zone_id]);
                    // XXX
                }
            }
        );
    });

    return await p;
}

function getNowPlaying(now_playing_info) {
    let info = {
        is_seek_allowed:  true,
        is_pause_allowed: true,
        image_url:        'https://i.scdn.co/image/' + now_playing_info.covers[0],
    }

    if (now_playing_info.album_name || !now_playing_info.show_name) {
        info.one_line = {
            line1: `${now_playing_info.name} - ${now_playing_info.artists.join('/')}`,
        };
        info.two_line = {
            line1: `${now_playing_info.name}`,
            line2: `${now_playing_info.artists.join(' / ')}`,
        };
        info.three_line = {
            line1: `${now_playing_info.name}`,
            line2: `${now_playing_info.artists.join(' / ')}`,
            line3: `${now_playing_info.album_name}`,
        };
    } else if (now_playing_info.show_name) {
        info.one_line = {
            line1: `${now_playing_info.name} - ${now_playing_info.show_name}`,
        };
        info.two_line = {
            line1: `${now_playing_info.name}`,
            line2: `${now_playing_info.show_name}`
        };
        info.three_line = {
            line1: `${now_playing_info.name}`,
            line2: `${now_playing_info.show_name}`,
            line3: ''
        };
    } else {
        info.one_line   = { line1: "Unknown" };
        info.two_line   = { line1: "Unknown" };
        info.three_line = { line1: "Unknown" };
    }
    return info;
}


const getSlots = zone_id => {
    return zoneSlots[zone_id] = zoneSlots[zone_id] || { play: null, queue: null };
}

let id = 0;
const inc = () => {
    return id++;
}



const handle_play_slot_event = (event, body, slots, zone_id) => {
    if (event == "OnToNext") {
        host.send_roon_message({
            type: 'OnToNext',
            id:   zone_id,
        });
    } else if (event == "Time") {
        host.send_roon_message({
            type:        'Time',
            id:          zone_id,
            seek_position_ms: body.seek_position_ms || 0,
            track_id:         slots.play.track_id
        });
    } else if (event == "Playing") {
        host.send_roon_message({
            type:        'Playing',
            id:          zone_id,
        });
    } else if (event == "Paused") {
        host.send_roon_message({
            type:        'Paused',
            id:          zone_id,
        });
    } else if (event == "Unpaused") {
        host.send_roon_message({
            type:        'Unpaused',
            id:          zone_id,
        });
    } else if (event == "EndedNaturally") {
        if (slots.queue) {
            host.send_roon_message({
                type: 'OnToNext',
                id:   zone_id,
            });
            zoneSlots[zone_id] = null;
        } else {
            host.send_roon_message({
                type:        'Stopped',
                id:          zone_id,
            });
            zoneSlots[zone_id] = null;
        }
    } else if (event == "MediaError") {
        host.send_roon_message({
            type:        'Stopped',
            id:          zone_id,
        });
        zoneSlots[zone_id] = null;
    } else if (event == "StoppedUser") {
        host.send_roon_message({
            type:        'Stopped',
            id:          zone_id,
        });
        zoneSlots[zone_id] = null;
    }
}

const handle_queue_slot_event = (event, body, slots, zone_id) => {
    if (event == 'Playing') {
        slots.queue.playing = true;
        host.send_roon_message({
            type:        'Playing',
            id:          zone_id,
        });
    } else {
        console.log('UNHANDLED QUEUE SLOT EVENT', 'id:', slots.queue.id, event);
    }
}



async function spotify_tells_us_to_play({
    zone_id,
    now_playing_info,
    position_ms,
    play_request_id,
    preload_id,
}) {

    const slots = getSlots(zone_id);

    // XXX Can we make use of spotify play_request_id?
    if (slots.queue?.playing && slots.queue?.preload_id === preload_id) {
        logger.info('Queue slot started playing succesfully, ignoring and clearing queue slot');
        slots.play  = slots.queue;
        slots.queue = null;
        return;
    }

    logger.info('spotify told us to play ' + zone_id);
    const session_id = await getOrCreateSession(zone_id);
    const info       = getNowPlaying(now_playing_info);

    let _slot_id = inc();

    const play_body = {
        session_id,
        track_id: now_playing_info.track_id,
        type: "track",
        slot: "play",
        media_url: `http://${librespot_http_url}:${librespot_http_port}/stream/${zone_id}/${now_playing_info.track_id}`,
        seek_position_ms: position_ms,
        info
    };

    slots.play = { ...play_body, id: _slot_id, play_request_id, preload_id };

    global_core.services.RoonApiAudioInput.play(play_body, (msg, body) => {
        if (!msg) return;
        const event = msg.name;

        console.log(`${_slot_id}:PLAYSLOT`, now_playing_info.track_id, msg)

        // We have moved on from this track -- disregard
        if (slots.play?.id !== _slot_id) return;

        handle_play_slot_event(event, body, slots, zone_id);
    })
    
}
async function spotify_tells_us_to_preload({ zone_id, now_playing_info, preload_id }) {
    const slots = getSlots(zone_id);

    logger.info('spotify told us to preload ' + zone_id);
    const session_id = await getOrCreateSession(zone_id);
    const info       = getNowPlaying(now_playing_info);
    const play_body  = {
        session_id,
        track_id: now_playing_info.track_id,
        type: "track",
        slot: "queue",
        media_url: `http://${librespot_http_url}:${librespot_http_port}/stream/${zone_id}/${now_playing_info.track_id}`,
        seek_position_ms: 0,
        info
    }

    let _slot_id = inc();

    slots.queue = { ...play_body, id: _slot_id, preload_id };
    global_core.services.RoonApiAudioInput.play(play_body,
        (msg, body) => {
        if (!msg) {
            console.log(`${_slot_id}:QUEUE EMPTY MSG`, now_playing_info.track_id, msg)
            return;
        };

        console.log(`${_slot_id}:QUEUE`, now_playing_info.track_id, msg)

        const event = msg.name;

        // Still in queue slot
        if (_slot_id === slots.queue?.id) {
            handle_queue_slot_event(event, body, slots, zone_id);
        // In play slot
        } else if (_slot_id === slots.play?.id) {
            handle_play_slot_event(event, body, slots, zone_id);
        } else {
            // Dont care
        }


    })
}

async function spotify_tells_us_to_clear({ zone_id, slots }) {
    logger.info({msg: 'Got clear from spotify', zone_id, slots });
    const session_id = await getOrCreateSession(zone_id);
    global_core.services.RoonApiAudioInput.clear({ session_id, slots });
}
function spotify_tells_us_to_seek({ zone_id, seek_position_ms }) {
    logger.info({msg: 'Got seek from spotify', zone_id, seek_position_ms});
    global_core.services.RoonApiTransport.seek(zone_id, 'absolute', seek_position_ms / 1000);
}
function spotify_tells_us_to_pause({ zone_id }) {
    logger.info({msg: 'Got pause from spotify', zone_id });
    global_core.services.RoonApiTransport.control(zone_id, "pause");
}
function spotify_tells_us_to_unpause({ zone_id }) {
    logger.info({msg: 'Got unpause from spotify', zone_id});
    global_core.services.RoonApiTransport.control(zone_id, "play");
}
function spotify_tells_us_to_stop({ zone_id }) {
    logger.info({msg: 'Got stop from spotify', zone_id});
    global_core.services.RoonApiTransport.control(zone_id, "stop");
}
function spotify_tells_us_to_set_volume({zone_id, volume}) {
    logger.info({msg: 'Got set volume from spotify', zone_id, volume});
    if (!sessions[zone_id]) {
        logger.info('ignoring volume request, session not started');
        return;
    }
    const scaledVol = volume / 65536; // Spotify sends value between 0 and 65535
    let zone = zones[zone_id];
    if (zone && zone.outputs.length == 1 && zone.outputs[0].volume && zone.outputs[0].volume.step) {
        const volumeHandle = zone.outputs[0].volume;
        if (volumeHandle.is_muted) {
            global_core.services.RoonApiTransport.mute(zone.outputs[0],'unmute');
        }
        global_core.services.RoonApiTransport.change_volume(zone.outputs[0],
            'absolute',
            Math.round(volumeHandle.min + (volumeHandle.max - volumeHandle.min) * scaledVol));
    } else {
        logger.info("VOLUME SETTING NOT SUPPORTED ON GROUPED ZONES");
    }
}
