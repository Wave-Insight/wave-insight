use std::{env, io::Error};

use futures_util::{future, StreamExt, TryStreamExt, SinkExt};
use tokio::net::{TcpListener, TcpStream};

use tungstenite::Message;
use wave_insight_lib::{
    parser::vcd_parser::vcd_parser,
    parser::verilog_parser::verilog_parser, data_struct::Module};
use std::io::Read;

#[tokio::main]
async fn main() -> std::result::Result<(), Error> {

    //warp::serve(warp::fs::dir("front-end/dist"))
    //    .run(([127, 0, 0, 1], 3030))
    //    .await;

    let mut file = std::fs::File::open("test.vcd").unwrap();
    let mut contents = String::new();
    file.read_to_string(&mut contents).unwrap();
    let (module_raw, signal_value_raw) = vcd_parser(&contents, &mut Module::new());
    let module = Box::new(module_raw);
    let signal_value = Box::new(signal_value_raw);

    let addr = env::args().nth(1).unwrap_or_else(|| "0.0.0.0:2992".to_string());

    // Create the event loop and TCP listener we'll accept connections on.
    let try_socket = TcpListener::bind(&addr).await;
    let listener = try_socket.expect("Failed to bind");
    println!("Listening on: {}", addr);

    while let Ok((stream, _)) = listener.accept().await {
        tokio::spawn(accept_connection(stream, module.clone()));
    }

    Ok(())
}

async fn accept_connection(stream: TcpStream, module: Box<Module>) {
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
        .forward(write)
        .await
        .expect("Failed to forward messages")
}
