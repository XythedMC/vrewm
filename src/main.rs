mod grabs;
mod handlers;
mod input;
mod ipc;
mod state;
mod winit;

pub use state::Treewm;

use smithay::reexports::{calloop::EventLoop, wayland_server::Display};
use tracing_subscriber::EnvFilter;

use crate::handlers::config::{create_config, read_config};

fn main() -> anyhow::Result<()>{
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .init();

    // Collect args: treewm [-e cmd [args...]]
    let args: Vec<String> = std::env::args().collect();
    let startup_cmd: Option<Vec<String>> = {
        let mut iter = args.iter().skip(1);
        if iter.next().map(|s| s == "-e").unwrap_or(false) {
            let rest: Vec<String> = args.iter().skip(2).cloned().collect();
            if rest.is_empty() { None } else { Some(rest) }
        } else {
            None
        }
    };

    let config = match read_config(){
        Ok(treewm_config) => treewm_config,
        Err(_) => {
            create_config()?;
            read_config().expect("Failed to read config after initial creation for some weird reason")
        }
    };

    let mut event_loop: EventLoop<Treewm> = EventLoop::try_new().expect("Failed to create event loop");
    let display: Display<Treewm> = Display::new().unwrap();

    let mut state = Treewm::new(&mut event_loop, display, config);

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

    let socket_str = state.socket_name.to_string_lossy().into_owned();

    // Propagate into our own environment so child processes inherit the right display.
    std::env::set_var("WAYLAND_DISPLAY",            &state.socket_name);
    std::env::set_var("MOZ_ENABLE_WAYLAND",         "1");  // Firefox / Zen
    std::env::set_var("GDK_BACKEND",                "wayland");
    std::env::set_var("QT_QPA_PLATFORM",            "wayland");
    std::env::set_var("ELECTRON_OZONE_PLATFORM_HINT", "wayland"); // Electron (Spotify, Claude …)
    std::env::set_var("CLUTTER_BACKEND",            "wayland");
    std::env::set_var("SDL_VIDEODRIVER",            "wayland");

    eprintln!("treewm: WAYLAND_DISPLAY={socket_str}");
    eprintln!("treewm: to run apps in this session:");
    eprintln!("  WAYLAND_DISPLAY={socket_str} <app>");
    eprintln!("  or start with:  treewm -e <terminal>");

    // Spawn the startup command (e.g. a terminal) with all Wayland env vars baked in.
    if let Some(cmd) = startup_cmd {
        let (prog, argv) = cmd.split_first().unwrap();
        match std::process::Command::new(prog).args(argv).spawn()
        {
            Ok(_)  => eprintln!("treewm: spawned: {}", cmd.join(" ")),
            Err(e) => eprintln!("treewm: failed to spawn '{}': {e}", cmd.join(" ")),
        }
    }

    event_loop
        .run(None, &mut state, |_| {})
        .expect("Event loop failed");
    Ok(())
}
