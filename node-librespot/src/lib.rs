use neon::{prelude::*};
use std::sync::{Arc, Mutex};
use std::cell::RefCell;
use tokio;
use tokio::sync::mpsc::{unbounded_channel};
use tokio::sync::mpsc::{UnboundedSender};
use std::thread;
use actix_web::{rt};
use serde::de::{DeserializeOwned};
use serde_json;
use neon::object::This;
use std::thread::JoinHandle;
use std::sync::mpsc::channel;
use actix_web::dev::ServerHandle;
#[macro_use] extern crate log;
extern crate simplelog;
use simplelog::*;
use std::fs::File;
use std::path::Path;
use std::env;
use std::process::exit;

mod playerinternal;
mod metadata;
mod zone;
mod spirc;
mod decrypt;
mod player;
mod server;
mod devices;

use zone::{SpotifyJSEvent, RoonMessage};
use devices::{HostMessage};

type BoxedHost = JsBox<RefCell<Host>>;

pub struct Host {
    server_url:           Option<String>,
    server_port:          Option<u16>,
    server_handle:        Option<ServerHandle>,
    server_thread_handle: Option<JoinHandle<()>>,
    devices_tx:           Option<UnboundedSender<RoonMessage>>,
    devices_handle:       Option<JoinHandle<()>>,
    host_devices_tx:      Option<UnboundedSender<HostMessage>>,
    js_callback:          Root<JsFunction>
}


impl Finalize for Host {
    fn finalize<'a, C: Context<'a>>(self, _cx: &mut C) {
        info!("CALLED FINALIZE");
    }
}

impl Host {
    fn new(base_url: Option<String>, listen_port: Option<u16>, callback: Root<JsFunction>) -> Self
    {
        Host {
            server_url:           base_url,
            server_port:          listen_port,
            server_handle:        None,
            server_thread_handle: None,
            devices_tx:           None,
            devices_handle:       None,
            host_devices_tx:      None,
            js_callback:          callback
        }
    }

    fn start(&mut self, this: Root<JsObject>, js_callback: Root<JsFunction>, jschannel: neon::event::Channel) {
        // Query track info from http server for each zone
        let (server_tx, server_rx) = channel();
        let (devices_server_tx, devices_server_rx) = unbounded_channel();


        // HTTP Server
        let port = self.server_port.clone();
        let url  = self.server_url.clone();
        let server_thread_handle = thread::spawn(move || {
            let server_future = server::run_server(
                    devices_server_tx,        // Send to devices thread
                    server_tx,                // Call back to server_rx from player thread
                    url,
                    port
                );
            rt::System::new().block_on(server_future).unwrap();
            info!("EXITED SERVER THREAD");
        });
        self.server_thread_handle = Some(server_thread_handle);
        let (server_handle, server_url, port) = server_rx.recv().unwrap();
        self.server_handle = Some(server_handle);
        self.server_url    = Some(server_url);
        self.server_port   = Some(port);

        let (host_devices_tx, devices_host_rx) = unbounded_channel();
        let (devices_tx, devices_rx)           = unbounded_channel();

        // Spotify 
        let devices_handle = thread::spawn(move || {
            let js_this_arc     = Arc::new(Mutex::new(this));
            let js_callback_arc = Arc::new(Mutex::new(js_callback));
            let devices_future  = devices::run(
                devices_rx,        // receive from roon
                devices_server_rx, // receive from http server
                devices_host_rx,   // receive shutdown command from host
                                   //
                                   // Call back into javascript event loop when spotify tells a
                                   // zone to do something
                                   //
                move |msg: SpotifyJSEvent| {
                    let js_this_arc2     = js_this_arc.clone();
                    let js_callback_arc2 = js_callback_arc.clone();
                    jschannel.send(move |mut c| {
                        let cb   = js_callback_arc2.lock().unwrap().to_inner(&mut c);
                        let s    = serde_json::to_string(&msg).unwrap();
                        let this = js_this_arc2.lock().unwrap().to_inner(&mut c);
                        let args = vec![c.string(&s).upcast()];
                        cb.call(&mut c, this, args).unwrap();
                        Ok(())
                    });
                }
            );
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(devices_future).unwrap();
        });

        self.devices_tx      = Some(devices_tx);
        self.host_devices_tx = Some(host_devices_tx);
        self.devices_handle  = Some(devices_handle);
    }

    fn send_roon_message(&mut self, msg: RoonMessage) {
        if self.devices_tx.is_some() {
            self.devices_tx.as_ref().unwrap()
            .send(msg).unwrap();
        }
    }

    fn stop(&mut self) {
        if self.host_devices_tx.is_some() {
            self.host_devices_tx.as_ref().unwrap()
                .send(HostMessage::Stop).unwrap();

            // Block until shutdown stops
            if let Some(handle) = self.devices_handle.take() {
                handle.join().unwrap();
                self.devices_handle  = None;
                self.host_devices_tx = None;
                self.devices_tx      = None;
            }

            // Block until server shuts down
            if let Some(handle) = self.server_handle.take() {
                futures_executor::block_on(handle.stop(true));
                self.server_handle = None;
                if let Some(thread_handle) = self.server_thread_handle.take() {
                    thread_handle.join().unwrap();
                    self.server_thread_handle = None;
                }
            }
        }
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        info!("DROPPED");
    }
}

// This will throw if deserialization fails
fn parse<C,T>(cx: &mut CallContext<C>) -> T
where
    C: This,
    T: DeserializeOwned
{
    let x    = cx.argument::<JsString>(0).unwrap();
    let s    = x.value(cx).clone();
    let t: T = serde_json::from_str(&s).unwrap();
    return t;
}

impl Host {

    fn js_new(mut cx: FunctionContext) -> JsResult<BoxedHost> {
        let base_url = cx.argument_opt(0);
        let url = match base_url {
            Some(p) => {
                match p.downcast::<JsString,_>(&mut cx) {
                    Ok(n) => Some(n.value(&mut cx)),
                    _ => None
                }
            },
            _ => None
        };

        let listen_port = cx.argument_opt(1);
        let port = match listen_port {
            Some(p) => {
                match p.downcast::<JsNumber,_>(&mut cx) {
                    Ok(n) => Some(n.value(&mut cx) as u16),
                    _ => None
                }
            },
            _ => None
        };

        let callback_function = cx.argument::<JsFunction>(2)?.root(&mut cx);
        let host = RefCell::new(Host::new(
                url,
                port,
                callback_function
        ));
        Ok(cx.boxed(host))
    }

    fn js_start(mut cx: FunctionContext) -> JsResult<JsPromise> {
        let host = cx.this().downcast_or_throw::<BoxedHost, _>(&mut cx)?;
        let mut host = host.borrow_mut();
        let channel = cx.channel();
        let callback = host.js_callback.clone(&mut cx);
        host.start(cx.this().root(&mut cx), callback, channel);
        let (deferred, promise) = cx.promise();
        deferred.settle_with(&cx.channel(), move |mut cx| Ok(cx.number(42)));
        Ok(promise)
    }

    fn js_stop(mut cx: FunctionContext) -> JsResult<JsPromise> {
        let host = cx.this().downcast_or_throw::<BoxedHost, _>(&mut cx)?;
        let mut host = host.borrow_mut();
        host.stop();
        let (deferred, promise) = cx.promise();
        deferred.settle_with(&cx.channel(), move |mut cx| Ok(cx.number(42)));
        Ok(promise)
    }

    fn js_port(mut cx: FunctionContext) -> JsResult<JsValue> {
        let host = cx.this().downcast_or_throw::<BoxedHost, _>(&mut cx)?;
        let host = host.borrow_mut();
        if let Some(port) = host.server_port {
            Ok(cx.number(port).as_value(&mut cx))
        } else {
            Ok(cx.null().as_value(&mut cx))
        }
    }

    fn js_url(mut cx: FunctionContext) -> JsResult<JsValue> {
        let host = cx.this().downcast_or_throw::<BoxedHost, _>(&mut cx)?;
        let host = host.borrow_mut();
        if let Some(url) = host.server_url.clone() {
            Ok(cx.string(url).as_value(&mut cx))
        } else {
            Ok(cx.null().as_value(&mut cx))
        }
    }

    fn js_send_roon_message(mut cx: FunctionContext) -> JsResult<JsPromise> {
        let host     = cx.this().downcast_or_throw::<BoxedHost, _>(&mut cx)?;
        let mut host = host.borrow_mut();
        let msg: RoonMessage = parse(&mut cx);
        host.send_roon_message(msg);
        let (deferred, promise) = cx.promise();
        deferred.settle_with(&cx.channel(), move |mut cx| Ok(cx.number(42)));
        Ok(promise)
    }

}

#[neon::main]
 fn main(mut cx: ModuleContext) -> NeonResult<()> {
     match env::current_exe() {
         Ok(exe_path) => { 
             let log_dir;
             if exe_path.ends_with("node") {
                 log_dir = Path::new("./").join("roon-extension-spotify-rs.log");
             } else {
                 log_dir = Path::new(&exe_path).parent().unwrap().join("roon-extension-spotify-rs.log");
                 println!("{}",log_dir.display());
             }
             // Set up logging
             CombinedLogger::init(
                 vec![
                     SimpleLogger::new(LevelFilter::Info, Config::default()),
                     WriteLogger::new(LevelFilter::Debug, Config::default(), File::create(log_dir).unwrap()),
                 ]
             ).unwrap();
         },
         Err(e) => {
             error!("Could not find log path");
             exit(1);
        },
     };

    cx.export_function("init",               Host::js_new)?;
    cx.export_function("stop",               Host::js_stop)?;
    cx.export_function("start",              Host::js_start)?;
    cx.export_function("send_roon_message",  Host::js_send_roon_message)?;
    cx.export_function("port",               Host::js_port)?;
    cx.export_function("url",                Host::js_url)?;
    Ok(())
 }
