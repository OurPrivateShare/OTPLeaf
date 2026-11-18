use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    path::PathBuf,
    rc::Rc,
    sync::mpsc,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use slint::winit_030::WinitWindowAccessor;
use slint::{ComponentHandle, Model, Timer, TimerMode, VecModel};

use crate::{
    AccountRow, AppWindow, EntryDraft,
    core::{Account, parse_otpauth},
    platform::{Tray, TrayAction, qr},
    storage::Store,
};

fn now() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .map_err(|_| "系统时间早于 1970 年，请检查时钟".to_owned())
}

fn display_code(code: &str) -> String {
    let (left, right) = code.split_at(code.len() / 2);
    format!("{left} {right}")
}

fn mask_secret(secret: &str) -> String {
    // The store only supplies normalized Base32, so byte indices are safe.
    let edge = (secret.len().saturating_sub(1) / 2).min(4);
    if edge == 0 {
        return "…".into();
    }
    format!("{}…{}", &secret[..edge], &secret[secret.len() - edge..])
}

enum CopyField {
    Code,
    Secret,
}

enum QrSource {
    Image(PathBuf),
    Screen,
}

type ScanResult = Result<Vec<String>, String>;

struct App {
    store: RefCell<Store>,
    rows: Rc<VecModel<AccountRow>>,
    tray: RefCell<Option<Tray>>,
    clipboard: RefCell<Option<arboard::Clipboard>>,
    pending_delete: RefCell<Option<String>>,
    import_queue: RefCell<VecDeque<Account>>,
    scan_sender: mpsc::Sender<ScanResult>,
    last_second: Cell<Option<u64>>,
    hidden_for_tray: Cell<bool>,
}

impl App {
    fn new(store: Store, tray: Option<Tray>, scan_sender: mpsc::Sender<ScanResult>) -> Self {
        Self {
            store: RefCell::new(store),
            rows: Rc::new(VecModel::default()),
            tray: RefCell::new(tray),
            clipboard: RefCell::new(None),
            pending_delete: Default::default(),
            import_queue: Default::default(),
            last_second: Default::default(),
            hidden_for_tray: Cell::new(false),
            scan_sender,
        }
    }

    fn refresh(&self, ui: &AppWindow, force: bool) {
        let time = match now() {
            Ok(time) => time,
            Err(error) => {
                self.rows.set_vec(Vec::new());
                self.last_second.set(None);
                ui.set_edit_error(error.into());
                return;
            }
        };
        let previous = self.last_second.replace(Some(time));
        if !force && previous == Some(time) {
            return;
        }
        let store = self.store.borrow();
        ui.set_total_count(store.accounts.len() as i32);
        let query = ui.get_query().trim().to_lowercase();
        let matches =
            |account: &&Account| query.is_empty() || account.name.to_lowercase().contains(&query);
        let mut count = 0;
        for (i, account) in store.accounts.iter().filter(matches).enumerate() {
            count = i + 1;
            let period = u64::from(account.period);
            let cached = self.rows.row_data(i);
            if !force
                && previous.is_some_and(|t| t / period == time / period)
                && let Some(mut row) = cached
                && row.id.as_str() == account.id
            {
                row.remaining = account.remaining_at(time) as i32;
                self.rows.set_row_data(i, row);
                continue;
            }
            let code = match account.code_at(time) {
                Ok(code) => display_code(&code),
                Err(error) => {
                    ui.set_edit_error(error.into());
                    String::new()
                }
            };
            let row = AccountRow {
                id: account.id.as_str().into(),
                name: account.name.as_str().into(),
                masked_secret: mask_secret(&account.secret).into(),
                algorithm: account.algorithm.to_string().into(),
                period: account.period as i32,
                remaining: account.remaining_at(time) as i32,
                digits: account.digits.into(),
                code: code.into(),
            };
            if i < self.rows.row_count() {
                self.rows.set_row_data(i, row);
            } else {
                self.rows.push(row);
            }
        }
        while self.rows.row_count() > count {
            self.rows.remove(self.rows.row_count() - 1);
        }
    }

    fn sync_accounts(&self, ui: &AppWindow) {
        if let Some(tray) = self.tray.borrow_mut().as_mut() {
            let entries = self
                .store
                .borrow()
                .accounts
                .iter()
                .map(|a| (a.id.clone(), a.name.clone()))
                .collect();
            tray.set_accounts(entries);
        }
        self.refresh(ui, true);
    }

    fn edit(&self, ui: &AppWindow, account: &Account, existing: bool) {
        ui.set_draft(EntryDraft {
            id: if existing {
                account.id.as_str().into()
            } else {
                Default::default()
            },
            name: account.name.as_str().into(),
            secret: account.secret.as_str().into(),
            digits: account.digits.to_string().into(),
            period: account.period.to_string().into(),
            algorithm: account.algorithm.to_string().into(),
        });
        ui.set_feedback(Default::default());
        ui.set_edit_error(Default::default());
        ui.set_page("edit".into());
    }

    fn new_entry(&self, ui: &AppWindow) {
        self.import_queue.borrow_mut().clear();
        self.edit(ui, &Account::default(), false);
    }

    fn show_next_import(&self, ui: &AppWindow) {
        let mut queue = self.import_queue.borrow_mut();
        if let Some(account) = queue.pop_front() {
            self.edit(ui, &account, false);
            let hint = if queue.is_empty() {
                "请核对导入的条目后保存".to_owned()
            } else {
                format!("保存后继续核对其余 {} 个条目，取消可结束导入", queue.len())
            };
            ui.set_feedback(hint.into());
        }
    }

    fn save_entry(&self, ui: &AppWindow, draft: EntryDraft) -> Result<(), String> {
        let mut entry = Account {
            name: draft.name.to_string(),
            secret: draft.secret.to_string(),
            digits: draft
                .digits
                .trim()
                .parse()
                .map_err(|_| "验证码位数须为 6、7 或 8")?,
            period: draft
                .period
                .trim()
                .parse()
                .map_err(|_| "周期须为正整数秒")?,
            algorithm: draft.algorithm.parse()?,
            ..Default::default()
        };
        let mut accounts = self.store.borrow().accounts.clone();
        if draft.id.is_empty() {
            accounts.push(entry);
        } else {
            let previous = accounts
                .iter_mut()
                .find(|a| a.id == draft.id.as_str())
                .ok_or("条目已不存在")?;
            entry.id.clone_from(&previous.id);
            *previous = entry;
        }
        self.store.borrow_mut().save_accounts(accounts)?;
        ui.set_edit_error(Default::default());
        self.sync_accounts(ui);
        if self.import_queue.borrow().is_empty() {
            ui.set_draft(Default::default());
            ui.set_page("accounts".into());
            ui.set_feedback("条目已保存".into());
        } else {
            self.show_next_import(ui);
        }
        Ok(())
    }

    fn delete(&self, ui: &AppWindow) -> Result<(), String> {
        let id = self
            .pending_delete
            .borrow()
            .clone()
            .ok_or("请先选择要删除的条目")?;
        let mut accounts = self.store.borrow().accounts.clone();
        accounts.retain(|a| a.id != id);
        self.store.borrow_mut().save_accounts(accounts)?;
        self.pending_delete.borrow_mut().take();
        self.sync_accounts(ui);
        ui.set_edit_error(Default::default());
        ui.set_page("accounts".into());
        ui.set_feedback("条目已删除".into());
        Ok(())
    }

    fn copy(&self, ui: &AppWindow, id: &str, field: CopyField) -> Result<(), String> {
        let store = self.store.borrow();
        let account = store
            .accounts
            .iter()
            .find(|a| a.id == id)
            .ok_or("条目已不存在")?;
        let (label, value) = match field {
            CopyField::Code => ("验证码", account.code_at(now()?)?),
            CopyField::Secret => ("密钥", account.secret.clone()),
        };
        let mut clipboard = self.clipboard.borrow_mut();
        if clipboard.is_none() {
            *clipboard =
                Some(arboard::Clipboard::new().map_err(|err| format!("无法打开剪贴板：{err}"))?);
        }
        clipboard
            .as_mut()
            .expect("clipboard initialized")
            .set_text(value)
            .map_err(|err| format!("复制失败：{err}"))?;
        ui.set_edit_error(Default::default());
        ui.set_feedback(format!("已复制「{}」的{label}", account.name).into());
        Ok(())
    }

    fn start_scan(&self, ui: &AppWindow, source: QrSource) -> Result<(), String> {
        if ui.get_busy() {
            return Ok(());
        }
        if ui.get_page() != "accounts" {
            self.show(ui);
            return Err("请关闭当前弹窗后再识别屏幕".into());
        }
        if matches!(source, QrSource::Screen) && ui.window().is_visible() {
            ui.hide().map_err(|err| format!("无法隐藏窗口：{err}"))?;
        }
        ui.set_edit_error(Default::default());
        ui.set_feedback(Default::default());
        ui.set_busy(true);
        let tx = self.scan_sender.clone();
        thread::spawn(move || {
            let result = match source {
                QrSource::Image(path) => qr::from_image(&path),
                QrSource::Screen => {
                    // Let the compositor finish hiding OTPLeaf before capturing.
                    thread::sleep(Duration::from_millis(450));
                    qr::from_screen()
                }
            };
            let _ = tx.send(result);
        });
        Ok(())
    }

    fn finish_scan(&self, ui: &AppWindow, result: ScanResult) -> Result<(), String> {
        ui.set_busy(false);
        self.show(ui);
        let links = result?;
        let mut entries = VecDeque::new();
        let mut last_error = None;
        for link in links {
            match parse_otpauth(&link) {
                Ok(account) => entries.push_back(account),
                Err(error) => last_error = Some(error),
            }
        }
        if entries.is_empty() {
            return Err(last_error.unwrap_or_else(|| "未找到 TOTP 二维码".into()));
        }
        *self.import_queue.borrow_mut() = entries;
        self.show_next_import(ui);
        if last_error.is_some() {
            ui.set_feedback("请核对有效条目。已跳过参数无效的二维码".into());
        }
        Ok(())
    }

    fn show(&self, ui: &AppWindow) {
        self.hidden_for_tray.set(false);
        if let Err(err) = ui.show() {
            ui.set_edit_error(err.to_string().into());
        }
        ui.window().set_minimized(false);
        ui.window()
            .with_winit_window(|window| window.focus_window());
    }
}

fn report(ui: &AppWindow, result: Result<(), String>) {
    if let Err(error) = result {
        ui.set_edit_error(error.into());
    }
}

fn bind(ui: &AppWindow, app: &Rc<App>) {
    // Keep callback state alive without creating a reference cycle with the window.
    macro_rules! on {
        ($callback:ident, |$app:ident, $ui:ident $(, $arg:ident)*| $body:expr) => {{
            let weak = ui.as_weak();
            let $app = Rc::clone(app);
            ui.$callback(move |$($arg),*| {
                if let Some(window) = weak.upgrade() {
                    let $ui = &window;
                    $body
                }
            });
        }};
    }
    on!(on_search, |app, ui, _query| app.refresh(ui, true));
    on!(on_add_entry, |app, ui| app.new_entry(ui));
    on!(on_cancel_edit, |app, ui| {
        app.import_queue.borrow_mut().clear();
        ui.set_draft(Default::default());
    });
    on!(on_edit_entry, |app, ui, id| {
        app.import_queue.borrow_mut().clear();
        if let Some(account) = app
            .store
            .borrow()
            .accounts
            .iter()
            .find(|a| a.id == id.as_str())
        {
            app.edit(ui, account, true);
        }
    });
    on!(on_save_entry, |app, ui, draft| report(
        ui,
        app.save_entry(ui, draft)
    ));
    on!(on_ask_delete, |app, ui, id| {
        if let Some(account) = app
            .store
            .borrow()
            .accounts
            .iter()
            .find(|a| a.id == id.as_str())
        {
            *app.pending_delete.borrow_mut() = Some(account.id.clone());
            ui.set_pending_name(account.name.as_str().into());
            ui.set_edit_error(Default::default());
            ui.set_feedback(Default::default());
            ui.set_page("delete".into());
        }
    });
    on!(on_confirm_delete, |app, ui| report(ui, app.delete(ui)));
    on!(on_copy_code, |app, ui, id| report(
        ui,
        app.copy(ui, id.as_str(), CopyField::Code)
    ));
    on!(on_copy_secret, |app, ui, id| report(
        ui,
        app.copy(ui, id.as_str(), CopyField::Secret)
    ));
    on!(on_import_uri, |app, ui, link| {
        app.import_queue.borrow_mut().clear();
        match parse_otpauth(link.as_str()) {
            Ok(account) => app.edit(ui, &account, false),
            Err(error) => ui.set_edit_error(error.into()),
        }
    });
    on!(on_import_image, |app, ui| {
        if let Some(path) = rfd::FileDialog::new()
            .set_title("选择 TOTP 二维码图片")
            .add_filter("二维码图片", &["png", "jpg", "jpeg", "bmp", "webp"])
            .pick_file()
        {
            report(ui, app.start_scan(ui, QrSource::Image(path)));
        }
    });
    on!(on_scan_screen, |app, ui| report(
        ui,
        app.start_scan(ui, QrSource::Screen)
    ));
    ui.on_quit_app(|| {
        let _ = slint::quit_event_loop();
    });
}

pub fn run() -> Result<(), String> {
    let store = Store::open_portable()?;
    let ui = AppWindow::new().map_err(|err| err.to_string())?;
    let (tray_tx, tray_events) = mpsc::channel();
    let (scan_sender, scans) = mpsc::channel();
    let tray = match Tray::new(tray_tx) {
        Ok(tray) => Some(tray),
        Err(error) => {
            ui.set_feedback(format!("托盘不可用，关闭窗口将最小化到任务栏。{error}").into());
            None
        }
    };
    let app = Rc::new(App::new(store, tray, scan_sender));
    ui.set_rows(app.rows.clone().into());
    app.sync_accounts(&ui);
    bind(&ui, &app);

    let weak = ui.as_weak();
    let state = app.clone();
    ui.window().on_close_requested(move || {
        if let Some(ui) = weak.upgrade() {
            if state.tray.borrow().as_ref().is_some_and(Tray::is_available) {
                state.hidden_for_tray.set(true);
                return slint::CloseRequestResponse::HideWindow;
            }
            ui.window().set_minimized(true);
        }
        slint::CloseRequestResponse::KeepWindowShown
    });
    let weak = ui.as_weak();
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_millis(150), move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        for action in tray_events.try_iter() {
            match action {
                TrayAction::Show => {
                    if !ui.get_busy() {
                        app.show(&ui);
                    }
                }
                TrayAction::ScanScreen => report(&ui, app.start_scan(&ui, QrSource::Screen)),
                TrayAction::Copy(id) => {
                    if let Err(error) = app.copy(&ui, &id, CopyField::Code) {
                        if !ui.get_busy() {
                            app.show(&ui);
                        }
                        ui.set_edit_error(error.into());
                    }
                }
                TrayAction::Quit => {
                    let _ = slint::quit_event_loop();
                    return;
                }
            }
        }
        for result in scans.try_iter() {
            report(&ui, app.finish_scan(&ui, result));
        }
        if !ui.get_busy()
            && app.hidden_for_tray.get()
            && !app.tray.borrow().as_ref().is_some_and(Tray::is_available)
        {
            app.show(&ui);
        }
        if ui.window().is_visible() && !ui.window().is_minimized() {
            app.refresh(&ui, false);
        }
    });
    ui.show().map_err(|err| err.to_string())?;
    slint::run_event_loop_until_quit().map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_workflow_through_window_callbacks() {
        i_slint_backend_testing::init_no_event_loop();
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open_dir(dir.path().into()).unwrap();
        let (sender, _) = mpsc::channel();
        let state = Rc::new(App::new(store, None, sender));
        let ui = AppWindow::new().unwrap();
        ui.set_rows(state.rows.clone().into());
        bind(&ui, &state);

        ui.invoke_add_entry();
        ui.invoke_save_entry(ui.get_draft());
        assert!(!ui.get_edit_error().is_empty());
        assert_eq!(ui.get_total_count(), 0);

        ui.invoke_import_uri("otpauth://totp/Example?secret=MY&period=45".into());
        let mut draft = ui.get_draft();
        assert_eq!(draft.period, "45");
        draft.secret = " jbsw y3dp ehpk 3pxp ".into();
        ui.invoke_save_entry(draft);
        assert_eq!(ui.get_page(), "accounts");
        let row = state.rows.row_data(0).unwrap();
        assert_eq!(row.name, "Example");
        assert_eq!(row.code.len(), 7);
        assert_eq!(row.masked_secret, "JBSW…3PXP");
        assert_eq!(state.store.borrow().accounts[0].secret, "JBSWY3DPEHPK3PXP");
        assert!((1..=45).contains(&row.remaining));

        for (query, count) in [("absent", 0), ("EXAM", 1)] {
            ui.set_query(query.into());
            ui.invoke_search(query.into());
            assert_eq!(state.rows.row_count(), count);
        }
        ui.invoke_edit_entry(row.id.clone());
        let mut draft = ui.get_draft();
        draft.name = "Updated".into();
        draft.secret = "MY".into();
        ui.invoke_save_entry(draft);
        assert_eq!(mask_secret("MZXW6YTB"), "MZX…YTB");
        assert_eq!(mask_secret("MY"), "…");
        assert_eq!(ui.get_total_count(), 1);

        ui.invoke_ask_delete(row.id);
        assert_eq!(ui.get_pending_name(), "Updated");
        assert_eq!(ui.get_total_count(), 1);
        ui.invoke_confirm_delete();
        assert_eq!(state.rows.row_count(), 0);
        drop(ui);
        drop(state);
        let reopened = Store::open_dir(dir.path().into()).unwrap();
        assert!(reopened.accounts.is_empty());
    }
}
