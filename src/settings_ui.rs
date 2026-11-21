//! Settings window (separate process, opened from the tray menu): a sidebar with General,
//! Hotkeys, Capture, Editor, Output, Updates (with What's new) and About.

use crate::config::{
    selectable_keys, Config, Hotkey, HotkeyState, HotkeyStatus, ImageFormat, PreviewQuality, ThemeChoice, VK_SNAPSHOT,
};
use crate::i18n::{t, tf, Language};
use crate::platform;
use crate::theme::{self, pal};
use eframe::egui::{self, vec2, Color32, RichText};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Opens the settings; `page` "whats-new" starts on the Updates page, scrolled to What's new.
pub fn run(page: Option<&str>) -> Result<(), String> {
    let whats_new = page == Some("whats-new");
    let page = if whats_new { Page::Updates } else { Page::General };
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(t("Blink settings"))
        .with_inner_size([820.0, 600.0])
        .with_min_inner_size([680.0, 440.0])
        .with_app_id("blink-settings");
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/blink.png")) {
        viewport = viewport.with_icon(Arc::new(icon));
    }
    let options = eframe::NativeOptions { viewport, centered: true, ..Default::default() };
    eframe::run_native(
        "Blink settings",
        options,
        Box::new(|cc| {
            crate::editor::setup_theme(&cc.egui_ctx);
            crate::editor::style_title_bar(cc);
            let cfg = Config::load();
            Ok(Box::new(SettingsApp {
                saved: cfg.clone(),
                cfg,
                page,
                scroll_to_whats_new: whats_new,
                status: None,
                hotkeys: HotkeyStatus::load(),
                hotkeys_read: Instant::now(),
                editing: None,
                recording: None,
                history_cleared: false,
                logo: crate::editor::logo_texture(&cc.egui_ctx),
            }))
        }),
    )
    .map_err(|e| e.to_string())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    General,
    Hotkeys,
    Capture,
    Editor,
    Output,
    Updates,
    About,
}

impl Page {
    const ALL: [Page; 7] =
        [Page::General, Page::Hotkeys, Page::Capture, Page::Editor, Page::Output, Page::Updates, Page::About];
    fn name(self) -> &'static str {
        match self {
            Page::General => t("General"),
            Page::Hotkeys => t("Hotkeys"),
            Page::Capture => t("Capture"),
            Page::Editor => t("Editor"),
            Page::Output => t("Output"),
            Page::Updates => t("Updates"),
            Page::About => t("About"),
        }
    }
}

struct SettingsApp {
    cfg: Config,
    /// The configuration as last saved (what the tray process uses).
    saved: Config,
    page: Page,
    /// Opened from "See what's new": scroll the Updates page to What's new once.
    scroll_to_whats_new: bool,
    status: Option<(String, bool)>,
    /// Per-hotkey registration results, written by the tray process.
    hotkeys: Option<HotkeyStatus>,
    hotkeys_read: Instant,
    /// The hotkey whose keys are being changed.
    editing: Option<&'static str>,
    /// The hotkey that takes the next key press.
    recording: Option<&'static str>,
    /// "Clear history" was pressed (shows that it worked).
    history_cleared: bool,
    logo: egui::TextureHandle,
}

/// A small "i" that explains a setting when clicked.
fn info(ui: &mut egui::Ui, id: &str, text: &str) {
    let (rect, resp) = ui.allocate_exact_size(vec2(18.0, 18.0), egui::Sense::click());
    let hovered = resp.hovered() || resp.has_focus();
    let p = ui.painter();
    let c = rect.center();
    p.circle_stroke(c, 7.5, egui::Stroke::new(1.2_f32, if hovered { pal().text } else { pal().muted }));
    p.text(
        c + vec2(0.0, 0.5),
        egui::Align2::CENTER_CENTER,
        "i",
        egui::FontId::new(11.0, egui::FontFamily::Name("ui_semibold".into())),
        if hovered { pal().text } else { pal().muted },
    );
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, t("More information")));
    let popup = ui.make_persistent_id(("info", id));
    if resp.clone().on_hover_text(t("What does this do?")).clicked() {
        ui.memory_mut(|m| m.toggle_popup(popup));
    }
    egui::popup_below_widget(ui, popup, &resp, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
        ui.set_max_width(320.0);
        ui.add(egui::Label::new(RichText::new(text).color(pal().text)).wrap());
    });
}

/// A small square button with the "opens in the browser" icon (a box with an arrow out of
/// its top-right corner), with `url` as its tooltip.
fn external_link(ui: &mut egui::Ui, url: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(22.0, 20.0), egui::Sense::click());
    let hovered = resp.hovered() || resp.has_focus();
    let p = ui.painter();
    if hovered {
        p.rect_filled(rect, theme::radius(20.0), pal().control_hover);
    }
    let c = if hovered { pal().text } else { pal().muted };
    let stroke = egui::Stroke::new(1.3_f32, c);
    let b = egui::Rect::from_center_size(rect.center(), vec2(11.0, 11.0));
    // The box, open at the top-right where the arrow leaves it.
    let (l, t_, r, bo) = (b.left(), b.top(), b.right(), b.bottom());
    p.line_segment([egui::pos2(l + 4.5, t_), egui::pos2(l, t_)], stroke);
    p.line_segment([egui::pos2(l, t_), egui::pos2(l, bo)], stroke);
    p.line_segment([egui::pos2(l, bo), egui::pos2(r, bo)], stroke);
    p.line_segment([egui::pos2(r, bo), egui::pos2(r, bo - 4.5)], stroke);
    // The arrow.
    let tip = egui::pos2(r + 1.0, t_ - 1.0);
    p.line_segment([egui::pos2(l + 4.5, bo - 4.5), tip], stroke);
    p.line_segment([tip, egui::pos2(tip.x - 4.5, tip.y)], stroke);
    p.line_segment([tip, egui::pos2(tip.x, tip.y + 4.5)], stroke);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, t("Open website")));
    resp.on_hover_text(url).on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A checkbox with an optional explanation.
fn check(ui: &mut egui::Ui, value: &mut bool, label: &str, help: Option<(&str, &str)>) {
    ui.horizontal(|ui| {
        ui.checkbox(value, label);
        if let Some((id, text)) = help {
            info(ui, id, text);
        }
    });
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(14.0);
    ui.label(RichText::new(title).family(egui::FontFamily::Name("ui_semibold".into())).size(15.0).color(pal().text));
    ui.add_space(4.0);
}

fn note(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).small().color(pal().muted)).wrap());
}

fn status_label(ui: &mut egui::Ui, color: Color32, text: &str) -> egui::Response {
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), egui::Sense::hover());
        ui.painter().circle_filled(r.center(), 4.0, color);
        ui.label(RichText::new(text).color(color));
    })
    .response
}

/// What the user can do about a hotkey's status.
enum HotkeyAction {
    None,
    Override(bool),
    OpenWindowsSetting,
}

fn hotkey_status(ui: &mut egui::Ui, hk: Hotkey, saved: Option<Hotkey>, state: Option<HotkeyState>) -> HotkeyAction {
    let mut action = HotkeyAction::None;
    if saved != Some(hk) {
        ui.label(RichText::new(t("Save to apply")).color(pal().muted));
        return action;
    }
    if !hk.is_enabled() {
        ui.label(RichText::new(t("Off")).color(pal().muted));
        return action;
    }
    if cfg!(windows) && !platform::resident_running() {
        ui.label(RichText::new(t("Not active")).color(pal().muted))
            .on_hover_text(t("Blink is not running, so no hotkeys work. Start Blink at the top of this page."));
        return action;
    }
    ui.horizontal(|ui| match state {
        None => {
            ui.label(RichText::new(t("Checking...")).color(pal().muted));
        }
        Some(HotkeyState::Disabled) => {
            ui.label(RichText::new(t("Off")).color(pal().muted));
        }
        Some(HotkeyState::Active) => {
            status_label(ui, pal().ok, t("Active"));
        }
        Some(HotkeyState::Overriding) => {
            status_label(ui, pal().accent_text, t("Active (overriding)"))
                .on_hover_text(t("Blink takes this combination before other apps that registered it."));
            if ui.small_button(t("Stop override")).clicked() {
                action = HotkeyAction::Override(false);
            }
        }
        Some(HotkeyState::InUse) => {
            let windows = hk.vk == VK_SNAPSHOT && platform::print_screen_opens_snipping();
            let text = if windows { t("In use by Windows (Snipping Tool)") } else { t("In use by another app") };
            status_label(ui, pal().warn, text);
            if ui
                .add(
                    egui::Button::new(RichText::new(t("Override")).strong().color(theme::INK))
                        .fill(theme::ACCENT)
                        .small(),
                )
                .on_hover_text(t("Take this hotkey anyway (saves the settings)"))
                .clicked()
            {
                action = HotkeyAction::Override(true);
            }
            if windows
                && ui
                    .small_button(t("Windows setting"))
                    .on_hover_text(t("Turn off \"Use the Print screen key to open screen capture\""))
                    .clicked()
            {
                action = HotkeyAction::OpenWindowsSetting;
            }
        }
    });
    action
}

/// Windows virtual-key code of an egui key, for the keys the hotkey list offers.
fn vk_of(key: egui::Key) -> Option<u32> {
    use egui::Key;
    let name = key.name();
    if name.len() == 1 {
        let c = name.chars().next()?.to_ascii_uppercase();
        if c.is_ascii_alphanumeric() {
            return Some(c as u32);
        }
    }
    if let Some(n) = name.strip_prefix('F').and_then(|n| n.parse::<u32>().ok()) {
        if (1..=24).contains(&n) {
            return Some(0x6F + n);
        }
    }
    match key {
        Key::Insert => Some(0x2D),
        Key::Home => Some(0x24),
        Key::End => Some(0x23),
        Key::PageUp => Some(0x21),
        Key::PageDown => Some(0x22),
        _ => None,
    }
}

impl eframe::App for SettingsApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // The tray process rewrites the status after every (re)registration.
        if self.hotkeys_read.elapsed() > Duration::from_millis(800) {
            self.hotkeys = HotkeyStatus::load();
            self.hotkeys_read = Instant::now();
        }
        ctx.request_repaint_after(Duration::from_secs(1));
        self.record_keys(ctx);
        egui::TopBottomPanel::bottom("buttons")
            .frame(egui::Frame::NONE.fill(pal().panel).inner_margin(egui::Margin::symmetric(16, 10)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let dirty = format!("{:?}", self.cfg) != format!("{:?}", self.saved);
                    let save =
                        egui::Button::new(RichText::new(t("Save")).strong().color(theme::INK)).fill(theme::ACCENT);
                    if ui.add_enabled(dirty, save).clicked() {
                        self.status = Some(match self.save() {
                            Ok(()) => (t("Saved. Hotkeys are active immediately.").into(), false),
                            Err(e) => (e, true),
                        });
                    }
                    if ui.button(t("Close")).clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    if ui.button(t("Restore defaults")).clicked() {
                        let keep = (
                            self.cfg.output_dir.clone(),
                            self.cfg.skipped_version.clone(),
                            self.cfg.last_version.clone(),
                            self.cfg.updated_at,
                        );
                        self.cfg = Config {
                            output_dir: keep.0,
                            skipped_version: keep.1,
                            last_version: keep.2,
                            updated_at: keep.3,
                            ..Config::default()
                        };
                        self.apply_look(ctx);
                    }
                    if let Some((msg, err)) = &self.status {
                        let c = if *err { pal().error } else { pal().ok };
                        ui.label(RichText::new(msg).color(c));
                    } else if dirty {
                        ui.label(RichText::new(t("Unsaved changes")).color(pal().muted));
                    }
                });
            });
        egui::SidePanel::left("nav")
            .resizable(false)
            .exact_width(170.0)
            .show_separator_line(false)
            .frame(egui::Frame::NONE.fill(pal().panel).inner_margin(egui::Margin::symmetric(10, 14)))
            .show(ctx, |ui| {
                crate::editor::brand_header(ui, &self.logo, "Blink", &format!("v{}", crate::update::CURRENT));
                ui.add_space(12.0);
                ui.spacing_mut().item_spacing.y = 4.0;
                for page in Page::ALL {
                    let on = self.page == page;
                    let text = RichText::new(page.name()).color(if on { theme::INK } else { pal().text });
                    let b = egui::Button::new(text)
                        .fill(if on { theme::ACCENT } else { Color32::TRANSPARENT })
                        .min_size(vec2(150.0, 30.0));
                    if ui.add(b).clicked() {
                        self.page = page;
                    }
                }
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(pal().panel_raised).inner_margin(egui::Margin::symmetric(22, 8)))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    // Room between the text and the scroll bar.
                    egui::Frame::NONE.inner_margin(egui::Margin { right: 18, ..Default::default() }).show(ui, |ui| {
                        ui.set_max_width(620.0);
                        match self.page {
                            Page::General => self.general(ui, ctx),
                            Page::Hotkeys => self.hotkeys_page(ui),
                            Page::Capture => self.capture(ui),
                            Page::Editor => self.editor(ui),
                            Page::Output => self.output(ui),
                            Page::Updates => self.updates(ui),
                            Page::About => self.about(ui),
                        }
                        ui.add_space(12.0);
                    });
                });
            });
    }
}

impl SettingsApp {
    /// Shows the language and theme choice in this window right away.
    fn apply_look(&self, ctx: &egui::Context) {
        self.cfg.apply_globals();
        crate::editor::setup_theme(ctx);
    }

    fn general(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        section(ui, t("Appearance"));
        let before = (self.cfg.language, self.cfg.theme);
        egui::Grid::new("look").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
            ui.label(t("Language"));
            egui::ComboBox::from_id_salt("lang").selected_text(self.cfg.language.native_name()).show_ui(ui, |ui| {
                for l in Language::ALL {
                    ui.selectable_value(&mut self.cfg.language, l, l.native_name());
                }
            });
            ui.end_row();
            ui.label(t("Theme"));
            ui.horizontal(|ui| {
                for (th, name) in [(ThemeChoice::Dark, t("Dark")), (ThemeChoice::Light, t("Light")), (ThemeChoice::System, t("Like Windows"))]
                {
                    ui.selectable_value(&mut self.cfg.theme, th, name);
                }
                info(ui, "theme", t("Dark or light windows for the editor, settings and update windows. \"Like Windows\" follows the app mode in the Windows color settings. The capture overlay always stays dark."));
            });
            ui.end_row();
        });
        if (self.cfg.language, self.cfg.theme) != before {
            self.apply_look(ctx);
        }
        note(ui, t("Open editor windows change the next time they open."));

        section(ui, t("After capture"));
        let c = &mut self.cfg;
        check(
            ui,
            &mut c.after_capture_ask,
            t("Ask me what to do"),
            Some(("ask", t("A small menu appears at the mouse pointer after each capture: open in editor, copy, or save. Esc discards the screenshot. The choices below are then not used."))),
        );
        ui.add_enabled_ui(!c.after_capture_ask, |ui| {
            ui.checkbox(&mut c.after_capture_open_editor, t("Open in editor"));
            ui.checkbox(&mut c.after_capture_copy, t("Copy to clipboard"));
            ui.checkbox(&mut c.after_capture_save, t("Save to output folder"));
        });

        section(ui, t("System"));
        ui.checkbox(&mut c.start_with_windows, t("Start Blink when Windows starts"));
        if cfg!(windows) && !platform::resident_running() {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(t("Blink is not running, so no hotkeys work.")).color(pal().warn));
                if ui.button(t("Start Blink")).clicked() {
                    if let Ok(exe) = std::env::current_exe() {
                        let _ = std::process::Command::new(exe).spawn();
                    }
                }
            });
        }
    }

    fn hotkeys_page(&mut self, ui: &mut egui::Ui) {
        section(ui, t("Hotkeys"));
        if cfg!(windows) && !platform::resident_running() {
            ui.horizontal(|ui| {
                ui.label(RichText::new(t("Blink is not running, so no hotkeys work.")).color(pal().warn));
                if ui.button(t("Start Blink")).clicked() {
                    if let Ok(exe) = std::env::current_exe() {
                        let _ = std::process::Command::new(exe).spawn();
                    }
                }
            });
            ui.add_space(6.0);
        }
        let mut action: Option<(&'static str, HotkeyAction)> = None;
        egui::Grid::new("hk").num_columns(4).spacing([14.0, 10.0]).show(ui, |ui| {
            for (id, name, _) in self.saved.hotkeys.list() {
                ui.label(name);
                let hk = *self.cfg.hotkeys.get_mut(id).expect("known hotkey");
                let recording = self.recording == Some(id);
                let chip = if recording { t("Press the keys...").to_string() } else { hk.label() };
                let chip = egui::Button::new(RichText::new(chip).monospace())
                    .fill(if recording { theme::ACCENT.gamma_multiply(0.25) } else { pal().input_bg })
                    .stroke(egui::Stroke::new(1.0_f32, if recording { theme::ACCENT } else { pal().border }))
                    .min_size(vec2(210.0, 26.0));
                if ui.add(chip).on_hover_text(t("Click and press the new key combination")).clicked() {
                    self.recording = if recording { None } else { Some(id) };
                }
                let open = self.editing == Some(id);
                if ui
                    .selectable_label(open, t("More..."))
                    .on_hover_text(t("Win key, Print Screen and other keys"))
                    .clicked()
                {
                    self.editing = if open { None } else { Some(id) };
                }
                let saved = self.saved.hotkeys.list().into_iter().find(|e| e.0 == id).map(|e| e.2);
                let entry = self.hotkeys.as_ref().and_then(|s| s.get(id)).filter(|e| Some(e.hotkey) == saved);
                let a = hotkey_status(ui, hk, saved, entry.map(|e| e.state));
                if !matches!(a, HotkeyAction::None) {
                    action = Some((id, a));
                }
                ui.end_row();
                if open {
                    ui.label("");
                    let hk = self.cfg.hotkeys.get_mut(id).expect("known hotkey");
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut hk.ctrl, "Ctrl");
                        ui.checkbox(&mut hk.alt, "Alt");
                        ui.checkbox(&mut hk.shift, "Shift");
                        ui.checkbox(&mut hk.win, "Win");
                        egui::ComboBox::from_id_salt(id)
                            .width(120.0)
                            .selected_text(crate::config::key_name(hk.vk))
                            .height(320.0)
                            .show_ui(ui, |ui| {
                                for (vk, name) in selectable_keys() {
                                    ui.selectable_value(&mut hk.vk, vk, name);
                                }
                            });
                    });
                    ui.end_row();
                }
            }
        });
        match action {
            Some((id, HotkeyAction::Override(on))) => {
                if let Some(h) = self.cfg.hotkeys.get_mut(id) {
                    h.force = on;
                }
                self.status = Some(match self.save() {
                    Ok(()) => (
                        if on {
                            t("Saved. Blink now takes this hotkey even though another app registered it.")
                        } else {
                            t("Saved. The hotkey is registered normally again.")
                        }
                        .into(),
                        false,
                    ),
                    Err(e) => (e, true),
                });
            }
            Some((_, HotkeyAction::OpenWindowsSetting)) => platform::open_keyboard_settings(),
            _ => {}
        }
        ui.add_space(8.0);
        note(ui, t("Click a combination and press the new keys. Print Screen, Pause, Scroll Lock and the Win key cannot be recorded: choose those under More. Backspace while recording turns a hotkey off."));
        ui.horizontal(|ui| {
            note(ui, t("\"In use\" means another app (or Windows) registered the same combination first."));
            info(ui, "inuse", t("Override makes Blink catch the keys before that app does; the other app then no longer gets them. Or pick another combination. On Windows 11, Print Screen opens the Snipping Tool unless you turn that off: use the \"Windows setting\" button."));
        });
    }

    /// Takes the next key press for the hotkey being recorded.
    fn record_keys(&mut self, ctx: &egui::Context) {
        let Some(id) = self.recording else { return };
        let mut done = false;
        ctx.input(|i| {
            for e in &i.events {
                if let egui::Event::Key { key, pressed: true, modifiers, .. } = e {
                    if *key == egui::Key::Escape {
                        done = true;
                    } else if *key == egui::Key::Backspace {
                        if let Some(hk) = self.cfg.hotkeys.get_mut(id) {
                            hk.vk = 0;
                        }
                        done = true;
                    } else if let Some(vk) = vk_of(*key) {
                        if let Some(hk) = self.cfg.hotkeys.get_mut(id) {
                            hk.vk = vk;
                            hk.ctrl = modifiers.ctrl;
                            hk.alt = modifiers.alt;
                            hk.shift = modifiers.shift;
                            hk.win = false;
                        }
                        done = true;
                    }
                }
            }
        });
        if done {
            self.recording = None;
        }
    }

    fn capture(&mut self, ui: &mut egui::Ui) {
        let c = &mut self.cfg;
        section(ui, t("Selecting"));
        check(
            ui,
            &mut c.show_magnifier,
            t("Show magnifier while selecting"),
            Some(("magnifier", t("A zoomed view of the pixels around the mouse pointer, to start and end a selection on exactly the right pixel."))),
        );
        section(ui, t("Mouse pointer"));
        check(
            ui,
            &mut c.capture_pointer,
            t("Include the mouse pointer"),
            Some(("pointer", t("Captures the mouse pointer where it is when you press the hotkey. In the editor it is a separate object: drag it elsewhere, or select it and press Delete. Copy and Save draw it into the screenshot. Not for captures started from the tray menu (the pointer would only show the menu) or scrolling captures."))),
        );
        section(ui, t("Scrolling capture"));
        egui::Grid::new("scroll").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
            ui.label(t("Wait per scroll step"));
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut c.scroll_delay_ms).range(30..=2000).suffix(" ms"));
                info(ui, "delay", t("How long Blink waits after each scroll step for the page to settle. Raise it for pages that load images or animate while scrolling; lower it for faster captures."));
            });
            ui.end_row();
            ui.label(t("Maximum height"));
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut c.scroll_max_height).range(1000..=100_000).suffix(" px"));
                info(ui, "maxh", t("Scrolling stops at this height. Very long pages take a lot of memory: each copy of a 1920 px wide page of 30,000 px is about 230 MB, and stitching and editing keep several copies."));
            });
            ui.end_row();
        });
        // Rough peak memory: about three full copies of the page while stitching and editing.
        let mb = 1920.0 * c.scroll_max_height as f64 * 4.0 * 3.0 / 1_048_576.0;
        if c.scroll_max_height > 30_000 {
            ui.label(
                RichText::new(tf("A page this long can use about {0} MB of memory (1920 px wide). On a PC with 8 GB that may be too much.", &[&format!("{mb:.0}")]))
                    .color(pal().warn),
            );
        }
        section(ui, t("History"));
        ui.horizontal(|ui| {
            ui.label(t("Keep the last"));
            ui.add(egui::DragValue::new(&mut self.cfg.history_size).range(0..=crate::history::MAX_KEEP));
            ui.label(t("screenshots"));
            info(ui, "history", t("Your latest screenshots, under History in the tray menu. Click one to copy it again, with the same Edit and Save buttons as after a capture. They are stored only on this PC, in your local app data folder. 0 turns History off and deletes what it kept."));
        });
        ui.horizontal(|ui| {
            if ui.button(t("Clear history")).clicked() {
                crate::history::clear();
                self.history_cleared = true;
            }
            if self.history_cleared {
                ui.label(RichText::new(t("History cleared.")).color(pal().muted));
            }
        });
    }

    fn editor(&mut self, ui: &mut egui::Ui) {
        let c = &mut self.cfg;
        section(ui, t("Closing"));
        ui.checkbox(&mut c.close_editor_after_copy, t("Close editor after copying"));
        ui.checkbox(&mut c.close_editor_after_save, t("Close editor after saving"));
        check(
            ui,
            &mut c.confirm_escape_close,
            t("Ask before Esc closes the editor"),
            Some(("esc", t("Esc first cancels what you are doing (drawing, a selection). When there is nothing left to cancel it closes the editor. With this on, Blink asks first. Unsaved annotations are always asked about."))),
        );
        section(ui, t("Toolbar"));
        check(
            ui,
            &mut c.show_tool_labels,
            t("Show labels under the tool icons"),
            Some(("labels", t("Writes the name under every tool. The toolbar gets taller; the tooltips and shortcuts stay the same."))),
        );
        section(ui, t("Preview quality"));
        ui.horizontal(|ui| {
            for (q, name) in [
                (PreviewQuality::Fast, t("Fast")),
                (PreviewQuality::Balanced, t("Balanced")),
                (PreviewQuality::Full, t("Full resolution")),
            ] {
                ui.selectable_value(&mut c.preview_quality, q, name);
            }
            info(ui, "quality", t("How sharp the editor draws a screenshot with a background or device mockup. Fast: half the screen resolution, the quickest response to changes. Balanced: exactly the screen resolution, sharp on screen. Full resolution: the whole output at its real size, the slowest. Copy and Save always use the full, original resolution, whatever you choose here."));
        });
        note(
            ui,
            match c.preview_quality {
                PreviewQuality::Fast => t("Quickest, a little soft on screen."),
                PreviewQuality::Balanced => t("Sharp on screen and quick. Recommended."),
                PreviewQuality::Full => t("Exact, but slow with large screenshots or long pages."),
            },
        );
    }

    fn output(&mut self, ui: &mut egui::Ui) {
        let c = &mut self.cfg;
        section(ui, t("Saving"));
        egui::Grid::new("save").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
            ui.label(t("Output folder"));
            ui.horizontal(|ui| {
                let mut s = c.output_dir.display().to_string();
                if ui.add(egui::TextEdit::singleline(&mut s).desired_width(260.0)).changed() {
                    c.output_dir = s.into();
                }
                if ui.button(t("Browse...")).clicked() {
                    if let Some(d) = rfd::FileDialog::new().set_directory(&c.output_dir).pick_folder() {
                        c.output_dir = d;
                    }
                }
                if ui.button(t("Open")).clicked() {
                    std::fs::create_dir_all(&c.output_dir).ok();
                    platform::open_folder(&c.output_dir);
                }
            });
            ui.end_row();
            ui.label(t("File name"));
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut c.filename_pattern).desired_width(260.0));
                    info(ui, "pattern", t("The name of each saved screenshot. %Y is the year, %m the month, %d the day, %H %M %S the time. Everything else is used as it is. If the file exists, Blink adds (2), (3), ..."));
                });
                let preview = c.next_output_path();
                note(
                    ui,
                    &tf("e.g. {0}", &[&preview.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()]),
                );
            });
            ui.end_row();
            ui.label(t("Format"));
            ui.horizontal(|ui| {
                for f in ImageFormat::ALL {
                    ui.selectable_value(&mut c.format, f, f.name());
                }
                if c.format == ImageFormat::Jpg {
                    ui.add(egui::Slider::new(&mut c.jpg_quality, 50..=100).text(t("quality")));
                }
                info(ui, "format", t("PNG: lossless and sharp, opens everywhere. The safe choice for screenshots with text and interface.\n\nJPG: smallest files for photos and gradients, but it blurs text a little and has no transparency. For screenshots of photos, or where only JPG is accepted.\n\nWebP: lossless like PNG, usually a quarter to a third smaller. Good for the web, documentation and chat. Most current apps open it, some older programs and upload forms do not.\n\nCopy always puts a lossless image on the clipboard."));
            });
            ui.end_row();
        });
    }

    fn updates(&mut self, ui: &mut egui::Ui) {
        section(ui, t("Updates"));
        ui.horizontal(|ui| {
            ui.label(tf("Version {0}", &[&crate::update::CURRENT]));
            if ui.button(t("Check for updates...")).clicked() {
                if let Ok(exe) = std::env::current_exe() {
                    let _ = std::process::Command::new(exe).arg("--update").spawn();
                }
            }
        });
        check(
            ui,
            &mut self.cfg.check_updates,
            t("Check for updates automatically (once a day)"),
            Some(("updates", t("Blink asks GitHub once a day whether there is a newer version, and shows a notification if there is. Nothing is installed without your click. No personal data or screenshots are sent."))),
        );
        if !self.cfg.skipped_version.is_empty() {
            ui.horizontal(|ui| {
                note(ui, &tf("Skipped version: {0}", &[&self.cfg.skipped_version]));
                if ui.small_button(t("Remind me again")).clicked() {
                    self.cfg.skipped_version.clear();
                }
            });
        }
        self.whats_new(ui);
    }

    fn about(&mut self, ui: &mut egui::Ui) {
        crate::editor::brand_header(ui, &self.logo, "Blink", &tf("Version {0}", &[&crate::update::CURRENT]));
        note(ui, t("Lightweight screenshot tool with an annotation editor for Windows."));
        section(ui, t("Privacy"));
        note(ui, t("Blink works on your PC. Screenshots are never uploaded: they only leave your PC when you paste or share them yourself. The only network traffic is the daily update check with GitHub (Updates page), which you can turn off."));
        section(ui, t("License"));
        note(ui, t("Blink is free, open-source software under the GNU General Public License, version 3 or later. The source code, the license and the notices of every component it includes are on GitHub."));
        let repo = format!("https://github.com/{}", crate::update::REPO);
        if external_link(ui, &repo).clicked() {
            ui.ctx().open_url(egui::OpenUrl::new_tab(&repo));
        }
        section(ui, t("Third-party software"));
        note(ui, t("Blink is built with these open-source components, under their licenses:"));
        ui.add_space(4.0);
        egui::Grid::new("notices").num_columns(3).spacing([16.0, 3.0]).show(ui, |ui| {
            for (name, license, url) in [
                ("egui / eframe", "MIT OR Apache-2.0", "https://github.com/emilk/egui"),
                ("image", "MIT OR Apache-2.0", "https://github.com/image-rs/image"),
                ("tiny-skia", "BSD-3-Clause", "https://github.com/linebender/tiny-skia"),
                ("resvg / usvg", "Apache-2.0 OR MIT", "https://github.com/linebender/resvg"),
                ("ab_glyph", "Apache-2.0", "https://github.com/alexheretic/ab-glyph"),
                ("serde / serde_json", "MIT OR Apache-2.0", "https://github.com/serde-rs/serde"),
                ("arboard", "MIT OR Apache-2.0", "https://github.com/1Password/arboard"),
                ("rfd", "MIT", "https://github.com/PolyMeilex/rfd"),
                ("chrono", "MIT OR Apache-2.0", "https://github.com/chronotope/chrono"),
                ("dirs", "MIT OR Apache-2.0", "https://github.com/dirs-dev/dirs-rs"),
                ("sha2", "MIT OR Apache-2.0", "https://github.com/RustCrypto/hashes"),
                ("windows-rs", "MIT OR Apache-2.0", "https://github.com/microsoft/windows-rs"),
                (
                    "Twemoji (emoji stamps)",
                    "CC-BY 4.0, (c) Twitter, Inc. and other contributors",
                    "https://github.com/jdecked/twemoji",
                ),
            ] {
                ui.label(RichText::new(name).color(pal().text));
                ui.label(RichText::new(license).color(pal().muted));
                if external_link(ui, url).clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                }
                ui.end_row();
            }
        });
    }

    fn whats_new(&mut self, ui: &mut egui::Ui) {
        ui.add_space(14.0);
        let heading = ui.label(
            RichText::new(t("What's new"))
                .family(egui::FontFamily::Name("ui_semibold".into()))
                .size(15.0)
                .color(pal().text),
        );
        ui.add_space(4.0);
        if std::mem::take(&mut self.scroll_to_whats_new) {
            heading.scroll_to_me(Some(egui::Align::TOP));
        }
        note(ui, t("What changed in each version of Blink."));
        ui.add_space(6.0);
        for (i, e) in crate::changelog::entries().iter().enumerate() {
            let current = e.version == crate::update::CURRENT;
            let mut title = tf("Version {0}", &[&e.version]);
            if current {
                title = format!("{title}  ({})", t("this version"));
            }
            let header = RichText::new(title).family(egui::FontFamily::Name("ui_semibold".into())).color(pal().text);
            egui::CollapsingHeader::new(header).id_salt(("version", e.version)).default_open(i == 0).show(ui, |ui| {
                if !e.date.is_empty() {
                    note(ui, e.date);
                }
                crate::update_ui::release_notes(ui, &e.notes);
                ui.add_space(4.0);
            });
        }
    }

    fn save(&mut self) -> Result<(), String> {
        let all = self.cfg.hotkeys.list().map(|e| e.2);
        for (i, a) in all.iter().enumerate() {
            if a.is_enabled() && all[i + 1..].iter().any(|b| b.same_combo(a)) {
                return Err(tf("{0} is assigned twice.", &[&a.label()]));
            }
        }
        self.cfg.save().map_err(|e| tf("Could not save settings: {0}", &[&e]))?;
        self.saved = self.cfg.clone();
        platform::set_autostart(self.cfg.start_with_windows)?;
        platform::notify_resident_reload();
        Ok(())
    }
}
