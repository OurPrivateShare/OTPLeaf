use super::{ICON_SIZE, TrayAction, icon_pixel};
use ksni::blocking::{Handle, TrayMethods};
use ksni::menu::{StandardItem, SubMenu};
use ksni::{Icon, MenuItem, OfflineReason};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::Sender,
};

pub struct Tray {
    handle: Handle<Indicator>,
    available: Arc<AtomicBool>,
}

impl Tray {
    pub fn new(sender: Sender<TrayAction>) -> Result<Self, String> {
        let available = Arc::new(AtomicBool::new(true));
        let handle = Indicator {
            sender,
            accounts: Vec::new(),
            available: available.clone(),
        }
        .spawn()
        .map_err(|e| format!("系统托盘不可用，请检查桌面的状态栏支持：{e}"))?;
        Ok(Self { handle, available })
    }

    pub fn set_accounts(&self, accounts: Vec<(String, String)>) {
        self.handle.update(|tray| tray.accounts = accounts);
    }

    pub fn is_available(&self) -> bool {
        self.available.load(Ordering::Relaxed) && !self.handle.is_closed()
    }
}

struct Indicator {
    sender: Sender<TrayAction>,
    accounts: Vec<(String, String)>,
    available: Arc<AtomicBool>,
}

fn item(title: &str, action: TrayAction) -> MenuItem<Indicator> {
    StandardItem {
        label: title.into(),
        activate: Box::new(move |tray: &mut Indicator| {
            let _ = tray.sender.send(action.clone());
        }),
        ..Default::default()
    }
    .into()
}

impl ksni::Tray for Indicator {
    fn id(&self) -> String {
        "otpleaf".into()
    }

    fn title(&self) -> String {
        "OTPLeaf".into()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        let mut data = Vec::with_capacity((ICON_SIZE * ICON_SIZE * 4) as usize);
        for y in 0..ICON_SIZE {
            for x in 0..ICON_SIZE {
                let [r, g, b, a] = icon_pixel(x, y);
                data.extend_from_slice(&[a, r, g, b]);
            }
        }
        vec![Icon {
            width: ICON_SIZE,
            height: ICON_SIZE,
            data,
        }]
    }

    fn activate(&mut self, _: i32, _: i32) {
        let _ = self.sender.send(TrayAction::Show);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            item("显示窗口", TrayAction::Show),
            item("识别屏幕", TrayAction::ScanScreen),
            SubMenu {
                label: "复制验证码".into(),
                enabled: !self.accounts.is_empty(),
                submenu: self
                    .accounts
                    .iter()
                    .map(|(id, name)| item(&name.replace('_', "__"), TrayAction::Copy(id.clone())))
                    .collect(),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            item("退出", TrayAction::Quit),
        ]
    }

    fn watcher_offline(&self, _: OfflineReason) -> bool {
        self.available.store(false, Ordering::Relaxed);
        let _ = self.sender.send(TrayAction::Show);
        true
    }

    fn watcher_online(&self) {
        self.available.store(true, Ordering::Relaxed);
    }
}
