#![cfg_attr(not(test), windows_subsystem = "windows")]
#![cfg_attr(not(windows), allow(dead_code))]

mod annot;
mod capture_bar;
mod changelog;
mod clip_ui;
mod config;
mod editor;
mod emoji;
mod frame;
#[cfg(test)]
mod golden;
mod hdr;
mod history;
mod i18n;
mod i18n_de;
mod i18n_es;
mod i18n_fr;
mod i18n_nl;
mod i18n_pt;
mod imageio;
mod naming;
mod ocr;
mod pin_bar;
mod pin_ui;
mod platform;
mod qr;
mod rec;
mod render;
mod sensitive;
mod settings_ui;
mod stitch;
mod table;
mod theme;
mod toast_xml;
mod update;
mod update_ui;
mod welcome_ui;

use std::path::PathBuf;

fn main() {
    platform::init_process();
    // Language and light / dark theme, for every kind of process.
    config::Config::load().apply_globals();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |i: usize| args.get(i).map(String::as_str);

    let result = match arg(0) {
        Some("--edit-raw") => match arg(1) {
            Some(p) => {
                let path = PathBuf::from(p);
                let shot = imageio::read_capture(&path).map_err(|e| e.to_string());
                std::fs::remove_file(&path).ok();
                let select_text = args.iter().any(|a| a == "--select-text");
                let meta = naming::Meta::from_args(&args);
                shot.and_then(|(img, pointer)| editor::run_with(img, pointer, None, select_text, meta))
            }
            None => Err(i18n::t("Missing file path.").into()),
        },
        // A pinned screenshot. The optional x and y (physical pixels) are where it was taken,
        // so the pin covers that spot.
        Some("--pin-raw") => match arg(1) {
            Some(p) => {
                let path = PathBuf::from(p);
                let at = arg(2).zip(arg(3)).and_then(|(x, y)| Some((x.parse().ok()?, y.parse().ok()?)));
                // The tray shows pins itself, at once; this window is for when it is not running.
                if platform::pin_in_resident(&path, at, &naming::Meta::from_args(&args)) {
                    return;
                }
                let img = imageio::read_capture_flat(&path).map_err(|e| e.to_string());
                std::fs::remove_file(&path).ok();
                img.and_then(|img| pin_ui::run(img, at, naming::Meta::from_args(&args)))
            }
            None => Err(i18n::t("Missing file path.").into()),
        },
        Some("--pin") => match arg(1) {
            Some(p) => imageio::load_image_file(std::path::Path::new(p))
                .and_then(|img| pin_ui::run(img, None, naming::Meta::default())),
            None => Err(i18n::t("Missing file path.").into()),
        },
        Some("--open") => match arg(1) {
            Some(p) => {
                let path = PathBuf::from(p);
                imageio::load_image_file(&path).and_then(|img| editor::run(img, Some(path)))
            }
            None => Err(i18n::t("Missing file path.").into()),
        },
        Some("--open-dialog") => {
            let picked = rfd::FileDialog::new()
                .set_title(i18n::t("Open image in Blink"))
                .add_filter(i18n::t("Images"), &["png", "jpg", "jpeg", "bmp", "gif", "webp", "ico"])
                .add_filter(i18n::t("All files"), &["*"])
                .pick_file();
            match picked {
                Some(path) => imageio::load_image_file(&path).and_then(|img| editor::run(img, Some(path))),
                None => Ok(()),
            }
        }
        Some("--clipboard") => imageio::image_from_clipboard().and_then(|img| editor::run(img, None)),
        Some("--settings") => settings_ui::run(arg(1)),
        // "Save as" on the screenshot notification: asks where, then saves the hand-off file.
        // If saving fails the screenshot opens in the editor, so it is not lost.
        Some("--save-raw-as") => match arg(1) {
            Some(p) => {
                let raw = PathBuf::from(p);
                let c = config::Config::load();
                let result = match imageio::read_capture_flat(&raw) {
                    Err(e) => Err(e.to_string()),
                    Ok(img) => match imageio::ask_save_path(
                        &c,
                        &naming::Meta::from_args(&args).sized(img.width(), img.height()),
                    ) {
                        None => Ok(()),
                        Some(out) => {
                            let fmt = imageio::format_from_path(&out, c.format);
                            match imageio::save_image(&out, &img, fmt, c.jpg_quality) {
                                Ok(()) => Ok(()),
                                Err(e) => {
                                    platform::show_error("Blink", &e);
                                    std::fs::remove_file(&raw).ok();
                                    return editor::run(img, None)
                                        .unwrap_or_else(|e| platform::show_error("Blink", &e));
                                }
                            }
                        }
                    },
                };
                std::fs::remove_file(&raw).ok();
                result
            }
            None => Err(i18n::t("Missing file path.").into()),
        },
        // Screen recording: the recorder (started by the tray), the clip window, "As GIF" from
        // the notification, and a three-second test recording for new machines.
        Some("--record") => match arg(1) {
            Some(p) => platform::run_recorder(std::path::Path::new(p)),
            None => Err(i18n::t("Missing file path.").into()),
        },
        Some("--clip") => match arg(1) {
            Some(p) => clip_ui::run(std::path::Path::new(p)),
            None => Err(i18n::t("Missing file path.").into()),
        },
        Some("--export-gif") => match arg(1) {
            Some(p) => platform::export_gif(std::path::Path::new(p)),
            None => Err(i18n::t("Missing file path.").into()),
        },
        Some("--record-test") => platform::record_test(),
        Some("--update") => update_ui::run(),
        Some("--welcome") => welcome_ui::run(),
        Some(p) if !p.starts_with("--") && std::path::Path::new(p).is_file() => {
            // "Open with Blink" from Explorer.
            let path = PathBuf::from(p);
            imageio::load_image_file(&path).and_then(|img| editor::run(img, Some(path)))
        }
        _ => platform::run_resident(),
    };

    if let Err(e) = result {
        platform::show_error("Blink", &e);
    }
}
