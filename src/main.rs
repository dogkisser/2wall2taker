#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![feature(let_chains)]
#![warn(clippy::pedantic, clippy::nursery, clippy::style)]

mod walltaker;
mod ui {
    fl2rust_macro::include_ui!("src/ui.fl");
}

use fltk::{button::CheckButton, input::Input, prelude::*};
use std::{
    fs, io,
    path::{Path, PathBuf},
    collections::HashSet, net::TcpStream, num::ParseIntError, sync::mpsc,
};
use anyhow::{anyhow, Context, Result};
use tungstenite::{stream::MaybeTlsStream, WebSocket};
use tray_icon::{menu, TrayIcon, TrayIconBuilder};
use serde::{Serialize, Deserialize};
use single_instance::SingleInstance;

type Writer = WebSocket<MaybeTlsStream<TcpStream>>;

// 256 x 256
const ICON: &[u8] = include_bytes!("../assets/eggplant.rgb8");

#[derive(Default, Serialize, Deserialize)]
struct Settings {
    ids: HashSet<usize>,
    run_on_boot: bool,
}

macro_rules! dialog {
    ($($t:tt)*) => {{
        let centre = centre();
        fltk::dialog::alert(centre.0, centre.1, &format!($($t)*));
    }};
}

fn main() {
    dialog!("{:?}", app_main());
    std::process::exit(1);
}

#[expect(clippy::too_many_lines, reason = "Don't care rn")]
fn app_main() -> Result<()> {
    let _app = fltk::app::App::default();

    let instance = SingleInstance::new("2wall2taker").unwrap();
    if !instance.is_single() {
        dialog!("2 Wall 2 Taker is already running.");
    }

    let theme = fltk_theme::WidgetTheme::new(fltk_theme::ThemeType::Metro);
    theme.apply();

    let dir_base = directories::ProjectDirs::from("", "", "2wall2taker")
        .context("Couldn't create data directory")?;

    fs::create_dir_all(dir_base.config_dir())?;
    fs::create_dir_all(dir_base.data_dir())?;

    let wallpaper_save_path = dir_base.data_dir().join("2wall2taker-current");
    let settings_path = dir_base.config_dir().join("settings.json");
    let mut settings = read_settings(&settings_path);

    let (settings_write, settings_read) = mpsc::channel();

    let _tray = create_tray();

    let (mut ws_stream, _) = tungstenite::connect("wss://walltaker.joi.how/cable")?;
    if let MaybeTlsStream::NativeTls(stream) = ws_stream.get_mut() {
        stream.get_mut().set_nonblocking(true)?;
    }

    loop {
        if let Ok(menu::MenuEvent { id: menu::MenuId(id) })
            = menu::MenuEvent::receiver().try_recv()
        {
            match id.as_str() {
                "settings" => {
                    settings_window(&settings, settings_write.clone());
                },
                "save_cur" => {
                    let downloads_dir = directories::UserDirs::new()
                        .and_then(|d| d.download_dir().map(Path::to_path_buf))
                        .unwrap_or_default();
                    let downloads_dir = downloads_dir.to_string_lossy();
                    if let Some(dest) = fltk::dialog::file_chooser("Save Current Wallpaper",
                        "", &*downloads_dir, false)
                    && let Err(e) = fs::copy(&wallpaper_save_path, dest)
                    {
                        dialog!("Couldn't save wallpaper: {e:?}");
                    }
                },
                "data_dir" => {
                    // idk why there isn't a function to do this
                    let data_dir = dir_base.data_dir();
                    #[cfg(target_os = "windows")]
                    let data_dir = data_dir.parent().unwrap();
                    open::that(data_dir)?;
                },
                "refresh" => {
                    if let Some(id) = settings.ids.iter().next() {
                        walltaker::check(&mut ws_stream, *id)?;
                    } else {
                        dialog!("No links are set");
                    }
                },
                "exit" => {
                    std::process::exit(0);
                },
                _ => { },
            }
        }

        if let Ok(new) = settings_read.try_recv() {
            let removed_ids = settings.ids.difference(&new.ids);
            let added_ids = new.ids.difference(&settings.ids);

            for id in removed_ids {
                walltaker::unsubscribe_from(&mut ws_stream, *id)?;
            }

            for id in added_ids {
                walltaker::subscribe_to(&mut ws_stream, *id)?;
            }

            match startup_dir() {
                Ok(dir) => {
                    let this_bin = std::env::current_exe()?;
                    /* SAFETY: this is definitely a non-empty path */
                    let bin_name = this_bin.file_name().unwrap();
                    let rob_start = dir.join(bin_name);

                    if new.run_on_boot {
                        fs::create_dir_all(&dir)?; 
                        fs::copy(this_bin, rob_start)?;
                    } else {
                        match fs::remove_file(&rob_start) {
                            // deleted successfully or file didn't exist
                            Ok(()) => { },
                            Err(e) if e.kind() == io::ErrorKind::NotFound => { },
                            Err(e) => {
                                dialog!("Couldn't disable run on boot: {e:?}\nCheck {rob_start:#?}");
                            },
                        }
                    }
                },
                Err(e) => {
                    dialog!("Couldn't determine system startup dir: {e:?}");
                }
            }

            settings = new;
            save_settings(&settings, &settings_path)?;
        }

        if let Ok(message) = ws_stream.read() {
            let message = message.into_text()?;
            if let Err(e) = read_walltaker_message(
                &mut ws_stream,
                &settings,
                &wallpaper_save_path,
                &message
            ) {
                dialog!("Couldn't communicate with walltaker: {e:?}");
            }
        }

        fltk::app::wait_for(0.05)?;
    }
}

#[cfg(target_os = "windows")]
fn startup_dir() -> Result<PathBuf> {
    let app_data = PathBuf::from(&std::env::var("APPDATA")?);
    Ok(app_data.join("Microsoft/Windows/Start Menu/Programs/Startup"))
}

#[cfg(target_os = "linux")]
fn startup_dir() -> Result<PathBuf> {
    let config_home = PathBuf::from(&std::env::var("XDG_CONFIG_HOME")?);
    Ok(config_home.join("autostart"))
}

#[cfg(target_os = "macos")]
fn startup_dir() -> Result<PathBuf> {
    anyhow!("There is no global startup directory on macOS")
}

fn create_tray() -> Result<TrayIcon> {
    let menu = menu::Menu::new();
    menu.append(&menu::MenuItem::with_id("settings", "Settings", true, None))?;
    menu.append(&menu::MenuItem::with_id("save_cur", "Save Current Wallpaper", true, None))?;
    menu.append(&menu::MenuItem::with_id("data_dir", "Open Data Dir", true, None))?;
    menu.append(&menu::MenuItem::with_id("refresh", "Refresh", true, None))?;
    menu.append(&menu::MenuItem::with_id("exit", "Exit", true, None))?;

    Ok(TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_icon(tray_icon::Icon::from_rgba(ICON.to_vec(), 256, 256)?)
        .with_tooltip("2 Wall 2 Taker")
        .build()?)
}

fn read_settings<P: AsRef<Path>>(settings: P) -> Settings {
    fs::read_to_string(settings)
        .context("Reading settings file")
        .and_then(|s| serde_json::from_str::<Settings>(&s)
        .context("Reading settings JSON"))
        .unwrap_or_default()
}

fn save_settings<P: AsRef<Path>>(settings: &Settings, path: P) -> Result<()> {
    fs::write(path, serde_json::to_string(&settings)?)?;
    Ok(())
}

fn centre() -> (i32, i32) {
    #[expect(clippy::cast_possible_truncation, reason = "Why would a pixel count be fractional")]
    (
        (fltk::app::screen_size().0 / 2.0) as i32,
        (fltk::app::screen_size().1 / 2.0) as i32,
    )
}

fn read_walltaker_message<P: AsRef<Path>>(
    writer: &mut Writer,
    settings: &Settings,
    wallpaper_save_path: &P,
    message: &str
) -> Result<()> {
    use crate::walltaker::Incoming;

    println!("{message}");
    let message = serde_json::from_str(message)?;
    #[expect (clippy::match_same_arms, reason = "Prettier, might diverge later")]
    match message {
        Incoming::Ping { .. } => { },
        Incoming::ConfirmSubscription { .. } => { },
        Incoming::Disconnect { .. } => { /* todo */ },
        Incoming::Welcome => {
            for id in &settings.ids {
                walltaker::subscribe_to(writer, *id)?;
            }
        },
        Incoming::Message { message, .. } => {
            if let Some(url) = message.post_url {
                // it's important that out_file is dropped before setting the
                // wallpaper to make windows happy.
                {
                    let mut out_file = fs::File::create(wallpaper_save_path)?;

                    let config = ureq::config::Config::builder()
                        .tls_config(ureq::tls::TlsConfig::builder()
                            .provider(ureq::tls::TlsProvider::NativeTls)
                            .build()
                        ).build();
                    let agent = config.new_agent();

                    let mut response = agent.get(url).call()?;
                    let mut content = response.body_mut().as_reader();

                    std::io::copy(&mut content, &mut out_file)?;
                }

                let path_str = wallpaper_save_path.as_ref().to_string_lossy();
                wallpaper::set_from_path(&path_str)
                    .map_err(|_| anyhow!("setting wallpaper"))?;
                wallpaper::set_mode(wallpaper::Mode::Fit)
                    .map_err(|_| anyhow!("couldn't set wallpaper mode"))?;
            }
        },
    }

    Ok(())
}

fn settings_window(
    current_settings: &Settings,
    settings_write: mpsc::Sender<Settings>
) {
    let mut ui: ui::UserInterface = ui::UserInterface::settings_window();

    let ids = current_settings.ids
        .iter()
        .map(|s| ToString::to_string(&s))
        .collect::<Vec<String>>()
        .join(" ");

    ui.ids_input.set_value(&ids);
    ui.run_on_boot.set_checked(current_settings.run_on_boot);

    ui.save_but.set_callback(move |b| {
        /* SAFETY: definitely exists */
        let ids: Input = fltk::app::widget_from_id("ids_input").unwrap();
        let Ok(ids): Result<HashSet<usize>, ParseIntError> = ids.value()
            .split_whitespace()
            .map(str::parse)
            .collect() else {
                dialog!("Invalid link ID(s) input");
                return;
            };

        /* SAFETY: definitely exists */
        let run_on_boot: CheckButton = fltk::app::widget_from_id("run_on_boot").unwrap();
        let run_on_boot = run_on_boot.is_checked();

        /* SAFETY: channel will definitely be open */
        settings_write.send(Settings { ids, run_on_boot, }).unwrap();
        /* SAFETY: widget definitely still has a parent as it was just pressed */
        b.window().unwrap().hide();
    });

    ui.cancel_but.set_callback(|b| {
        /* SAFETY: widget definitely still has a parent as it was just pressed */
        b.window().unwrap().hide();
    });
}
