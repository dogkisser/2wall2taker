#![warn(clippy::pedantic, clippy::style)]
use anyhow::{Context, Result};
use fltk::prelude::*;
use futures::{poll, stream::SplitSink, StreamExt};
use tokio_tungstenite::{tungstenite::Message, MaybeTlsStream, WebSocketStream};
use std::{fs, io::Cursor, task::Poll::Ready};
use tokio::{io::AsyncWriteExt, net::TcpStream};
use tray_icon::{menu, TrayIconBuilder, TrayIconEvent};

mod walltaker;

type Writer = SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>;

// 256 x 256
const ICON: &[u8] = include_bytes!("../assets/eggplant.rgb8");

mod ui {
    fl2rust_macro::include_ui!("src/ui.fl");
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Settings {
    ids: Vec<usize>,
    run_on_boot: bool,
}

fn read_settings<P: AsRef<std::path::Path>>(settings: P) -> Settings {
    fs::read_to_string(settings)
        .context("Reading settings file")
        .and_then(|s| serde_json::from_str::<Settings>(&s).context("Reading settings JSON"))
        .unwrap_or_default()
}

async fn save_settings<P: AsRef<std::path::Path>>(settings: &Settings, path: P) -> Result<()> {
    tokio::fs::write(path, serde_json::to_string(&settings).unwrap()).await?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let app = fltk::app::App::default();

    let dir_base = directories::ProjectDirs::from("", "", "2wall2taker")
        .context("Couldn't create data directory")?;
    fs::create_dir_all(dir_base.config_dir())?;

    let settings_path = dir_base.config_dir().join("settings.json");
    let mut settings = read_settings(&settings_path);

    let (tray_send, mut tray_recv) = tokio::sync::mpsc::unbounded_channel();
    let menu = menu::Menu::new();
    menu.append(&menu::MenuItem::with_id("settings", "Settings", true, None))?;
    menu.append(&menu::MenuItem::with_id("exit", "Exit", true, None))?;
    menu::MenuEvent::set_event_handler(Some(move |event| {
        println!("send {event:?}");
        tray_send.send(event).unwrap();
    }));

    let tray_icon = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_icon(tray_icon::Icon::from_rgba(ICON.to_vec(), 256, 256)?)
        .with_tooltip("2wall2taker")
        .build()?;

    let (settings_write, mut settings_read) = tokio::sync::mpsc::unbounded_channel();

    // let (ws_stream, _) = tokio_tungstenite::connect_async("wss://walltaker.joi.how/cable").await?;
    // let (mut write, mut read) = ws_stream.split();

    // settings_window(&settings, settings_write.clone());

    loop {
        tokio::select! {
            Some(new) = settings_read.recv() => {
                settings = new;
                save_settings(&settings, &settings_path).await?;
            },
            Some(menu::MenuEvent { id: menu::MenuId(id) }) = tray_recv.recv() => {
                println!("got");
                match id.as_str() {
                    "settings" => {
                        settings_window(&settings, settings_write.clone());
                    },
                    "exit" => {
                        std::process::exit(0);
                    },
                    _ => { },
                }
            },
        };

        fltk::app::wait_for(0.1).unwrap();
        // message from walltaker
        // if let Ready(Some(Ok(message))) = poll!(read.next()) {
        //     if let Err(e) = read_walltaker_message(&dir_base, &message.into_text()?).await {
        //         println!("{e:?}");
        //     }
        // }
    }
}

fn settings_window(current_settings: &Settings, settings_write: tokio::sync::mpsc::UnboundedSender<Settings>) {
    let mut ui: ui::UserInterface = ui::UserInterface::settings_window();

    let ids = current_settings.ids
        .iter()
        .map(|s| ToString::to_string(&s))
        .collect::<Vec<String>>()
        .join(" ");
    ui.ids_input.set_value(&ids);
    

    ui.ids_input.set_id("ids_input");

    ui.run_on_boot.set_checked(current_settings.run_on_boot);

    ui.save_but.set_callback(move |_b| {
        let ids: fltk::input::Input = fltk::app::widget_from_id("ids_input").unwrap();
        let ids = ids.value().split_whitespace().map(|x| x.parse().unwrap()).collect::<Vec<usize>>();

        let run_on_boot: fltk::button::CheckButton = fltk::app::widget_from_id("run_on_boot").unwrap();
        let run_on_boot = run_on_boot.is_checked();

        settings_write.send(Settings { ids, run_on_boot, }).unwrap();
    });
    ui.cancel_but.set_callback(|b| {
        b.window().unwrap().hide();
    });
}

async fn read_walltaker_message(
    dir_base: &directories::ProjectDirs,
    message: &str
) -> Result<()> {
    use crate::walltaker::Incoming;

    println!("{message}");
    let message = serde_json::from_str(message)?;
    #[allow(clippy::match_same_arms)]
    match message {
        Incoming::Ping { .. } => { },
        Incoming::ConfirmSubscription { .. } => { },
        Incoming::Disconnect { .. } => { /* todo */ },
        Incoming::Welcome => { /* todo, set initial wallpaper */ },
        Incoming::Message { message, .. } => {
            if let Some(url) = message.post_url {
                let data_dir = dir_base.data_dir();
                tokio::fs::create_dir_all(&data_dir).await?;

                let out = data_dir.join("lwwtc.jpg");
                let mut out_file = tokio::fs::File::create(&out).await?;

                let response = reqwest::get(url).await?;
                let mut content = Cursor::new(response.bytes().await?);

                tokio::io::copy(&mut content, &mut out_file).await?;
                out_file.flush().await?;

                wallpaper::set_from_path(out.to_string_lossy().as_ref())
                    .map_err(|_| anyhow::anyhow!("setting wallpaper"))?;
            }
        },
    }

    Ok(())
}