use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::{broadcast, oneshot};

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "cmd")]
pub enum IpcCommand {
    #[serde(rename = "get_tree")]
    GetTree,
    #[serde(rename = "focus")]
    Focus { id: String },
    #[serde(rename = "pan")]
    Pan { dx: f64, dy: f64 },
    #[serde(rename = "set_mode")]
    SetMode { mode: String },
}

pub enum InternalCommand {
    GetTree { reply_to: oneshot::Sender<String> },
    Focus { id: String },
    Pan { dx: f64, dy: f64 },
    SetMode { mode: String },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "event")]
pub enum IpcEvent {
    #[serde(rename = "window_opened")]
    WindowOpened { id: String, parent: Option<String> },
    #[serde(rename = "window_closed")]
    WindowClosed { id: String },
    #[serde(rename = "focus_changed")]
    FocusChanged { id: Option<String> },
    #[serde(rename = "layout_changed")]
    LayoutChanged,
    #[serde(rename = "mode_changed")]
    ModeChanged { mode: String },
    #[serde(rename = "viewport_changed")]
    ViewportChanged { x: f64, y: f64 },
}

#[derive(Serialize, Debug, Clone)]
pub struct TreeWindow {
    pub id: String,
    pub title: String,
    pub parent: Option<String>,
    pub children: Vec<String>,
    pub canvas_x: f64,
    pub canvas_y: f64,
    pub width: i32,
    pub height: i32,
    pub focused: bool,
}

#[derive(Serialize, Debug, Clone)]
pub struct TreeViewport {
    pub x: f64,
    pub y: f64,
}

#[derive(Serialize, Debug, Clone)]
pub struct TreeResponse {
    pub windows: Vec<TreeWindow>,
    pub viewport: TreeViewport,
    pub mode: String,
}

pub async fn run_ipc_server(
    cmd_tx: smithay::reexports::calloop::channel::Sender<InternalCommand>,
    event_rx: broadcast::Receiver<IpcEvent>,
    socket_path: &Path,
) {
    if socket_path.exists() {
        let _ = std::fs::remove_file(socket_path);
    }

    let listener = match UnixListener::bind(socket_path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Failed to bind IPC socket: {}", e);
            return;
        }
    };

    loop {
        let (mut socket, _) = match listener.accept().await {
            Ok(res) => res,
            Err(_) => continue,
        };

        let cmd_tx = cmd_tx.clone();
        let mut event_rx = event_rx.resubscribe();

        tokio::spawn(async move {
            let (read_half, mut write_half) = socket.split();
            let mut reader = BufReader::new(read_half);
            let mut line = String::new();

            loop {
                tokio::select! {
                    res = reader.read_line(&mut line) => {
                        match res {
                            Ok(0) => break, // EOF
                            Ok(_) => {
                                if let Ok(cmd) = serde_json::from_str::<IpcCommand>(&line) {
                                    let internal_cmd = match cmd {
                                        IpcCommand::GetTree => {
                                            let (tx, rx) = oneshot::channel();
                                            let _ = cmd_tx.send(InternalCommand::GetTree { reply_to: tx });
                                            if let Ok(tree_json) = rx.await {
                                                let _ = write_half.write_all(format!("{}\n", tree_json).as_bytes()).await;
                                            }
                                            line.clear();
                                            continue;
                                        }
                                        IpcCommand::Focus { id } => InternalCommand::Focus { id },
                                        IpcCommand::Pan { dx, dy } => InternalCommand::Pan { dx, dy },
                                        IpcCommand::SetMode { mode } => InternalCommand::SetMode { mode },
                                    };
                                    let _ = cmd_tx.send(internal_cmd);
                                }
                                line.clear();
                            }
                            Err(_) => break,
                        }
                    }
                    Ok(event) = event_rx.recv() => {
                        if let Ok(json) = serde_json::to_string(&event) {
                            let _ = write_half.write_all(format!("{}\n", json).as_bytes()).await;
                        }
                    }
                }
            }
        });
    }
}
