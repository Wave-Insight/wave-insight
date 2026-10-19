mod args;

use std::io::{Error, Read};
use std::process;

use futures_util::{future, SinkExt, StreamExt, TryStreamExt};
use tokio::net::{TcpListener, TcpStream};

use tungstenite::Message;
use wave_insight_lib::{
    parser::vcd_parser::vcd_parser,
    //parser::verilog_parser::verilog_parser,
    data_struct::Module,
    data_struct::ModuleValue};

use args::Args;

#[tokio::main]
async fn main() -> std::result::Result<(), Error> {

    tokio::spawn(serve_html());

    let args = Args::new();
    let filename = args.vcd_path.unwrap_or_else(|| "test.vcd".to_string());
    let mut file = std::fs::File::open(&filename)
        .unwrap_or_else(|e| {
            eprintln!("can not open vcd file `{filename}`: {e}");
            process::exit(1)
        });
    let mut contents = String::new();
    file.read_to_string(&mut contents)
        .unwrap_or_else(|e| {
            eprintln!("can not read vcd file `{filename}`: {e}");
            process::exit(1)
        });
    let (module_raw, signal_value_raw) = vcd_parser(contents, &mut Module::new());
    let module = Box::new(module_raw);
    let signal_value = Box::new(signal_value_raw);

    let addr = args.addr.unwrap_or_else(|| "0.0.0.0:2993".to_string());

    // Create the event loop and TCP listener we'll accept connections on.
    let try_socket = TcpListener::bind(&addr).await;
    let listener = try_socket.expect("Failed to bind");
    println!("Listening on: {addr}");

    while let Ok((stream, _)) = listener.accept().await {
        tokio::spawn(accept_connection(stream, module.clone(), signal_value.clone()));
    }

    Ok(())
}

async fn accept_connection(stream: TcpStream, module: Box<Module>, signal_value: Box<ModuleValue>) {
    let addr = stream.peer_addr().expect("connected streams should have a peer address");
    println!("Peer address: {addr}");

    let ws_stream = tokio_tungstenite::accept_async(stream)
        .await
        .expect("Error during the websocket handshake occurred");

    println!("New WebSocket connection: {addr}");

    let (mut write, read) = ws_stream.split();
    // We should not forward messages other than text or binary.

    //`Box::leak` used to be called here, which leaked a whole module tree on every
    //connection just to get a `'static` reference, borrowing the box is enough
    let instantiated = match serde_json::to_string_pretty(&*module) {
        Ok(instantiated) => instantiated,
        Err(e) => {
            eprintln!("can not serialize module for {addr}: {e}");
            return;
        },
    };
    if let Err(e) = write.send(Message::Text(format!("module:{instantiated}"))).await {
        eprintln!("can not send module to {addr}: {e}");
        return;
    }

    read.try_filter(|msg| future::ready(msg.is_text() || msg.is_binary()))
        .map(|msg| msg_to_str(msg, &signal_value))
        .forward(write)
        .await
        .expect("Failed to forward messages")
}

///Answer one request of the client.
///
///The protocol is just `s:<signal key>`, the answer is `sig:<signal key>\x1d<json>`.
///Anything else used to panic and then to close the connection, now it is answered
///with an `error:` message which the front end simply ignores.
fn msg_to_str(msg: Result<Message, tungstenite::Error>, signal_value: &ModuleValue) -> Result<Message, tungstenite::Error> {
    let text = msg?.into_text()?;
    let key = match text.strip_prefix("s:") {
        Some(key) => key,
        None => {
            eprintln!("unsupported request: {text}");
            return Ok(Message::Text(format!("error:unsupported request `{text}`")));
        },
    };
    match signal_value.value.get(key) {
        Some(value) => {
            let sig = serde_json::to_string_pretty(value)
                .unwrap_or_else(|e| {
                    eprintln!("can not serialize signal `{key}`: {e}");
                    "null".to_string()
                });
            Ok(Message::Text(format!("sig:{}{}{}", key, 29u8 as char, sig)))
        },
        None => {
            eprintln!("unknown signal: {key}");
            Ok(Message::Text(format!("error:unknown signal `{key}`")))
        },
    }
}

async fn serve_html() {
    warp::serve(warp::fs::dir("front-end/dist"))
        .run(([127, 0, 0, 1], 2992))
        .await;
}
