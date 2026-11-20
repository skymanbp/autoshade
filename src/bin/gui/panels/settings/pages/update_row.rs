//! Settings → About's 「Updates」 row (v1.18.5): what the last check said,
//! 「Check now」, and — when a newer version is known — 「Open the download
//! page」 and 「Skip this version」; then the automatic-check switch and what
//! it does. Built from the same pieces as every other row (`form_row`,
//! `form_help`). The rules and the state live in `update_check.rs`.
//!
//! Only parsed version numbers reach the screen: never text from GitHub's
//! reply, and not the page address either (it opens in the browser).

use crate::update_check::UpdateRow;
use crate::*;
use autoshade::update::CheckError;

use super::{form_help, form_row};

impl AutoShadeApp {
    pub(super) fn settings_update_row(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        let mut check_now = false;
        let mut open = None;
        let mut skip = None;
        form_row(ui, tr(lang, "Updates"), |ui| {
            let u = &mut self.update;
            let status = match &u.row {
                UpdateRow::NotChecked => tr(lang, "Not checked yet.").to_owned(),
                UpdateRow::Checking => tr(lang, "Checking…").to_owned(),
                UpdateRow::UpToDate(v) => {
                    trf(lang, "You have the latest version ({version}).", &[("version", &v.to_string())])
                }
                UpdateRow::Newer(rel) => {
                    trf(lang, "Version {version} is available.", &[("version", &rel.version.to_string())])
                }
                UpdateRow::Failed(CheckError::Offline) => tr(lang, "Could not check: no connection.").to_owned(),
                UpdateRow::Failed(CheckError::Refused) => {
                    tr(lang, "Could not check: GitHub refused the request.").to_owned()
                }
                UpdateRow::Failed(CheckError::Unreadable) => {
                    tr(lang, "Could not check: GitHub's answer could not be read.").to_owned()
                }
            };
            // Level with the row's label, like a control a row tall.
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), row_h(ui)),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| ui.add(egui::Label::new(status).wrap()),
            );
            let newer = match &u.row {
                UpdateRow::Newer(rel) => Some(rel.clone()),
                _ => None,
            };
            let skipped = newer.as_ref().is_some_and(|rel| u.prefs.skipped.as_deref() == Some(&*rel.version.to_string()));
            ui.horizontal_wrapped(|ui| {
                check_now = action(ui, !u.checking(), tr(lang, "Check now")).clicked();
                if let Some(rel) = &newer {
                    if action(ui, true, tr(lang, "Open the download page")).clicked() {
                        open = Some(rel.page.clone());
                    }
                    if !skipped && action(ui, true, tr(lang, "Skip this version")).clicked() {
                        skip = Some(rel.version);
                    }
                }
            });
            if skipped {
                form_help(ui, tr(lang, "You chose to skip this version: the automatic check will not mention it again."));
            }
            // The switch shows what is in force: with the environment
            // variable set it is off and disabled, and the stored choice is
            // left as it was.
            if u.env_off {
                let mut off = false;
                check_if(ui, false, &mut off, tr(lang, "Check for updates automatically"));
                form_help(ui, tr(lang, "Turned off by the AUTOSHADE_NO_UPDATE_CHECK environment variable."));
            } else if check(ui, &mut u.prefs.auto, tr(lang, "Check for updates automatically")).changed() {
                self.prefs_save_due = true;
            }
            form_help(
                ui,
                tr(
                    lang,
                    "At most once a day, AutoShade asks GitHub which version is the latest. The request carries the program's name and version and nothing else; nothing is downloaded or installed.",
                ),
            );
        });
        if check_now {
            self.start_manual_update_check();
        }
        if let Some(page) = open {
            // The system browser, through eframe's own link handling.
            ui.ctx().open_url(egui::OpenUrl::new_tab(page));
        }
        if let Some(v) = skip {
            self.skip_update_version(v);
        }
    }
}
