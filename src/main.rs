mod grabs;
mod handlers;
mod input;
mod ipc;
mod state;
mod winit;

pub use state::Treewm;

use smithay::reexports::{calloop::EventLoop, wayland_server::Display};
use tracing_subscriber::EnvFilter;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .init();

    let mut event_loop: EventLoop<Treewm> = EventLoop::try_new().unwrap();
    let display: Display<Treewm> = Display::new().unwrap();

    let mut state = Treewm::new(&mut event_loop, display);

    let (cmd_tx, cmd_rx) = smithay::reexports::calloop::channel::channel::<ipc::InternalCommand>();
    let (event_tx, event_rx) = tokio::sync::broadcast::channel(16);

    state.event_tx = Some(event_tx);

    let socket_path = std::path::PathBuf::from("/tmp/treewm.sock");
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            ipc::run_ipc_server(cmd_tx, event_rx, &socket_path).await;
        });
    });

    event_loop.handle().insert_source(cmd_rx, |event, _, state| {
        if let smithay::reexports::calloop::channel::Event::Msg(cmd) = event {
            state.handle_ipc_cmd(cmd);
        }
    }).unwrap();

    winit::init_winit(&mut event_loop, &mut state).expect("Failed to initialize winit backend");

    std::env::set_var("WAYLAND_DISPLAY", &state.socket_name);
    eprintln!("treewm: WAYLAND_DISPLAY={}", state.socket_name.to_string_lossy());

    event_loop
        .run(None, &mut state, |_| {})
        .expect("Event loop failed");
}
