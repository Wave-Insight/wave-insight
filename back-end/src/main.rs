use std::{env, io::Error, collections::HashMap};

use futures_util::{future, StreamExt, TryStreamExt, SinkExt};
use num::BigUint;
use tokio::net::{TcpListener, TcpStream};

use tungstenite::Message;
use wave_insight_lib::{
    parser::vcd_parser::vcd_parser,
    parser::verilog_parser::verilog_parser, data_struct::Module};
use std::io::Read;

#[tokio::main]
async fn main() -> std::result::Result<(), Error> {

    tokio::spawn(serve_html());

    let mut file_args = std::env::args().nth(1);
    let filename = file_args.get_or_insert("test.vcd".to_string());
    let mut file = std::fs::File::open(filename).unwrap();
    let mut contents = String::new();
    file.read_to_string(&mut contents).unwrap();
    let (module_raw, signal_value_raw) = vcd_parser(&contents, &mut Module::new());
    let module = Box::new(module_raw);
    let signal_value = Box::new(signal_value_raw);

    let addr = env::args().nth(2).unwrap_or_else(|| "0.0.0.0:2993".to_string());

    // Create the event loop and TCP listener we'll accept connections on.
    let try_socket = TcpListener::bind(&addr).await;
    let listener = try_socket.expect("Failed to bind");
    println!("Listening on: {}", addr);

    while let Ok((stream, _)) = listener.accept().await {
        tokio::spawn(accept_connection(stream, module.clone(), signal_value.clone()));
    }

    Ok(())
}

async fn accept_connection(stream: TcpStream, module: Box<Module>, signal_value: Box<HashMap<String,Vec<(i32, BigUint)>>>) {
    let addr = stream.peer_addr().expect("connected streams should have a peer address");
    println!("Peer address: {}", addr);

    let ws_stream = tokio_tungstenite::accept_async(stream)
        .await
        .expect("Error during the websocket handshake occurred");

    println!("New WebSocket connection: {}", addr);

    let (mut write, read) = ws_stream.split();
    // We should not forward messages other than text or binary.
    
    let instantiated = serde_json::to_string_pretty(&Box::leak(module));
    write.send(Message::Text(format!("module:{}",instantiated.unwrap()))).await.expect("Failed to send module");//TODO:do not panic
    
    read.try_filter(|msg| future::ready(msg.is_text() || msg.is_binary()))
        .map(|msg| msg_to_str(msg, signal_value.clone()))
        .forward(write)
        .await
        .expect("Failed to forward messages")

    
}

fn msg_to_str(msg: Result<Message, tungstenite::Error>, signal_value: Box<HashMap<String,Vec<(i32, BigUint)>>>) -> Result<Message, tungstenite::Error> {
    let msg_text = msg.ok().and_then(|m| m.into_text().ok());
    let key = msg_text.and_then(|t| t.strip_prefix("s:").map(|tt| tt.to_string()));
    let is_sig_key = key.clone().and_then(|k| signal_value.get(&k));
    let get_value = is_sig_key;
    let sig = get_value.and_then(|v| serde_json::to_string_pretty(v).ok()).unwrap();//TODO:do not unwrap
    Ok(Message::Text(format!("sig:{}:{}", key.unwrap(), sig)))//TODO:do not unwrap
}

async fn serve_html() {
    warp::serve(warp::fs::dir("front-end/dist"))
        .run(([127, 0, 0, 1], 2992))
        .await;
}
