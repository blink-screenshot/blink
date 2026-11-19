#![cfg_attr(not(test), windows_subsystem = "windows")]
#![cfg_attr(not(windows), allow(dead_code))]

mod annot;
mod capture_bar;
mod changelog;
mod config;
mod editor;
mod emoji;
mod frame;
mod i18n;
mod i18n_nl;
mod imageio;
mod ocr;
mod platform;
mod render;
mod sensitive;
mod settings_ui;
mod stitch;
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
                let img = imageio::read_raw(&path).map_err(|e| e.to_string());
                std::fs::remove_file(&path).ok();
                let select_text = arg(2) == Some("--select-text");
                img.and_then(|img| editor::run_with(img, None, select_text))
            }
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
                let result = match imageio::read_raw(&raw) {
                    Err(e) => Err(e.to_string()),
                    Ok(img) => match imageio::ask_save_path(&c) {
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
