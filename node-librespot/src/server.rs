use actix_web::{
    body::{
        SizedStream
    },
    http::{
        header::{self, HeaderValue},
        StatusCode
    },
    get, web, App, HttpServer, Responder, HttpRequest, HttpResponse};
use http_range::HttpRange;
use std::sync::Mutex;
use std::sync::Arc;
use tokio::sync::mpsc::{UnboundedSender};
use std::sync::mpsc::channel;
use std::sync::mpsc::Sender;
use tokio;
use std::task::{Context, Poll};
use core::pin::Pin;
use futures_core::Stream;

use actix_web::dev::ServerHandle;
use actix_http_test::unused_addr;


struct ServerInternal {
    devices_tx: UnboundedSender<ServerMessage>
}


struct SpotifyStreamer {
    track_id:   String,
    zone_id:    String,
    readpos:    usize,
    file_size:  usize,
    devices_tx: UnboundedSender<ServerMessage>,
}

impl Stream for SpotifyStreamer {
    type Item = Result<actix_web::web::Bytes, actix_web::Error>;

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.readpos >= self.file_size {
            return Poll::Ready(None); // Stream ended
        }

        let mut _buff = vec![0u8; 32*1024 as usize];
        let buff      = Arc::new(Mutex::new(_buff));
        let (responder, receiver) = channel::<ServerReply>();

        // Ask devices to give you chunk of data
        self.devices_tx.send(ServerMessage::TrackRead {
            zone_id:   self.zone_id.clone(),
            track_id:  self.track_id.clone(),
            start:     self.readpos, 
            end:       self.readpos + 32768, // XXX
            out:       buff.clone(),
            responder
        }).unwrap();
        match receiver.recv()
        {
            Ok(msg) => match msg {
                ServerReply::TrackRead {
                    read_len,
                    ..
                } => {
                    self.readpos += read_len;
                    let unwrapped = buff.lock().unwrap();
                    Poll::Ready(Some(Ok(unwrapped[..read_len].to_vec().into())))
                },
                    _ => {
                        Poll::Ready(Some(Err(actix_web::error::ErrorInternalServerError("Received invalid message back from oneshot"))))
                    }
            },
            Err(_err) => {
                Poll::Ready(Some(Err(actix_web::error::ErrorInternalServerError("Error"))))
            }
        }
    }
}

pub enum ServerReply {
    Busy,
    NotFound,
    TrackInfo {
        file_size: usize,
        track_id:  String,
    },
    TrackRead {
        read_len: usize,
        file_size: usize,
        track_id:  String,
    }
}


#[derive(Debug)]
pub enum ServerMessage {
    TrackInfo {
        zone_id: String,
        track_id: String,
        responder: Sender<ServerReply>
    },
    TrackRead {
        zone_id: String,
        track_id: String,
        start:   usize,
        end:     usize,
        out:     Arc<Mutex<Vec<u8>>>,
        responder: Sender<ServerReply>
    }
}


#[get("/hello/{name}")]
async fn greet(name: web::Path<String>) -> impl Responder {
        format!("Hello {name}!")
}

use std::time::{SystemTime};

#[get("/stream/{zone_id}/{req_track_id}")]
async fn stream(
    req:  HttpRequest, 
    path: web::Path<(String,String)>, 
    data: web::Data<Mutex<ServerInternal>>
) -> HttpResponse {
    let (zone_id,req_track_id) = path.into_inner();
    let headers = req.headers();
        let start = SystemTime::now();
    info!("REQ: {start:?}");
    info!("REQ ZONE: {zone_id:?}");
    info!("REQ TRACK: {req_track_id:?}");
    info!("REQ HEADERS: {headers:?}");
    // XXX Roon doesnt use the range-end portion 
    // of the range request header, so always assuming start -> end of file
    // for now.
    let mut offset      = 0    as usize;
    if let Some(ranges) = req.headers().get(header::RANGE) {
        if let Ok(ranges_header) = ranges.to_str() {
            if let Ok(ranges) = HttpRange::parse(ranges_header, 10000000000) {
                let length = ranges[0].length as usize;
                offset     = ranges[0].start as usize;
                debug!("RANGE: {} - {}", offset, length);
            }
        }
    }

    let state = data.lock().unwrap();
    let (responder, receiver) = channel::<ServerReply>();
    state.devices_tx.send(ServerMessage::TrackInfo {
        zone_id:   zone_id.clone(),
        track_id:  req_track_id.clone(),
        responder
    }).unwrap();
    info!("TRACK INFO REQ");
    let file_size;
    match receiver.recv()
        {
            Ok(msg) => match msg
            {
                ServerReply::TrackInfo
                {
                    file_size: track_file_size,
                    ..
                } => {
                    file_size = track_file_size
                },
                _ => {
                    return HttpResponse::build(StatusCode::NOT_FOUND).finish(); 
                }
            }
            Err(_err) => {
                return HttpResponse::build(StatusCode::NOT_FOUND).finish(); 
            }
        }
    info!("TRACK INFO REQ DONE");

    let mut res = HttpResponse::build(StatusCode::OK);
    res.insert_header((
            header::CONTENT_ENCODING,
            HeaderValue::from_static("identity"),
            ));

    res.insert_header((
            header::CONTENT_TYPE,
            HeaderValue::from_static("audio/ogg"),
            ));

    res.insert_header((
            header::ACCEPT_RANGES,
            HeaderValue::from_static("bytes"),
            ));

    res.insert_header((
            header::CONTENT_RANGE,
            format!("bytes {}-{}/{}", offset, file_size - 1, file_size)));

    // 206 if not whole file
    if offset != 0 {
        res.status(StatusCode::PARTIAL_CONTENT);
    }

    return res
        .body(
        SizedStream::new((file_size - offset) as u64,
            SpotifyStreamer {
                file_size,
                track_id:   req_track_id.clone(),
                zone_id:    zone_id.clone(),
                readpos:    offset,
                devices_tx: state.devices_tx.clone()
            }
        )
    );
}


pub async fn run_server(
    devices_tx: UnboundedSender<ServerMessage>,
    server_tx:  Sender<(ServerHandle, String, u16)>,
    base_url:    Option<String>,
    listen_port: Option<u16>
    ) -> std::io::Result<()> {
    let server_internal = web::Data::new(
        Mutex::new(
            ServerInternal { devices_tx }
        )
    );
    
    let server_url  = match base_url { Some(url) => url, _ => "0.0.0.0".to_string() };
    let server_port = match listen_port { Some(port) => port, _ => unused_addr().port() };

    let server = HttpServer::new(move ||{
        App::new()
            .app_data(server_internal.clone())
            .route("/hello", web::get().to(|| async { "Hello World!" }))
            .service(stream)
    })
    .bind((server_url.clone(), server_port.clone()))?
        .disable_signals()
        .run();

    server_tx.send((server.handle(), server_url, server_port)).unwrap();
    server.await
}

