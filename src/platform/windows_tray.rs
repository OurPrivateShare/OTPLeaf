use super::{ICON_SIZE, TrayAction, icon_pixel};
use std::cell::{Cell, RefCell};
use std::ptr::{null, null_mut};
use std::sync::mpsc::Sender;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::{Shell::*, WindowsAndMessaging::*};
use windows_sys::core::w;

const TRAY_MESSAGE: u32 = WM_APP + 1;
const SHOW: usize = 1;
const ADD: usize = 2;
const QUIT: usize = 3;
const FIRST_ACCOUNT: usize = 4;

pub struct Tray {
    window: HWND,
    state: Box<State>,
}

struct State {
    sender: Sender<TrayAction>,
    accounts: RefCell<Vec<(String, String)>>,
    icon: HICON,
    taskbar_created: u32,
    available: Cell<bool>,
}

impl Tray {
    pub fn new(sender: Sender<TrayAction>) -> Result<Self, String> {
        // This hidden top-level window receives Shell callbacks and the
        // TaskbarCreated broadcast through the Slint thread's Win32 event loop.
        unsafe {
            let instance = GetModuleHandleW(null());
            let class = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: instance,
                lpszClassName: w!("OTPLeaf.TrayWindow"),
                ..Default::default()
            };
            RegisterClassW(&class);
            let window = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                class.lpszClassName,
                w!("OTPLeaf.Tray"),
                WS_POPUP,
                0,
                0,
                0,
                0,
                null_mut(),
                null_mut(),
                instance,
                null(),
            );
            if window.is_null() {
                return Err(format!(
                    "无法创建托盘窗口：{}",
                    std::io::Error::last_os_error()
                ));
            }
            let mut pixels = Vec::with_capacity((ICON_SIZE * ICON_SIZE * 4) as usize);
            for y in (0..ICON_SIZE).rev() {
                for x in 0..ICON_SIZE {
                    let [r, g, b, a] = icon_pixel(x, y);
                    pixels.extend_from_slice(&[b, g, r, a]);
                }
            }
            let mask = [0u8; (ICON_SIZE * ICON_SIZE / 8) as usize];
            let icon = CreateIcon(
                instance,
                ICON_SIZE,
                ICON_SIZE,
                1,
                32,
                mask.as_ptr(),
                pixels.as_ptr(),
            );
            let tray = Self {
                window,
                state: Box::new(State {
                    sender,
                    icon,
                    accounts: RefCell::default(),
                    taskbar_created: RegisterWindowMessageW(w!("TaskbarCreated")),
                    available: Cell::new(false),
                }),
            };
            SetWindowLongPtrW(
                window,
                GWLP_USERDATA,
                (&*tray.state as *const State) as isize,
            );
            if icon.is_null() || !tray.state.register(window) {
                return Err("Windows 通知区域暂时不可用".into());
            }
            Ok(tray)
        }
    }

    pub fn set_accounts(&self, accounts: Vec<(String, String)>) {
        self.state.accounts.replace(accounts);
    }

    pub fn is_available(&self) -> bool {
        self.state.available.get()
    }
}

impl State {
    fn icon_data(&self, window: HWND) -> NOTIFYICONDATAW {
        let mut tip = [0; 128];
        for (dst, unit) in tip.iter_mut().zip("OTPLeaf".encode_utf16()) {
            *dst = unit;
        }
        NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: window,
            uID: 1,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
            uCallbackMessage: TRAY_MESSAGE,
            hIcon: self.icon,
            szTip: tip,
            ..Default::default()
        }
    }

    fn register(&self, window: HWND) -> bool {
        let registered = unsafe { Shell_NotifyIconW(NIM_ADD, &self.icon_data(window)) } != 0;
        self.available.set(registered);
        registered
    }

    fn show_menu(&self, window: HWND) {
        // TrackPopupMenu runs a nested message loop. Snapshot the menu and IDs
        // before entering it, so pending edits cannot change a command's target.
        let accounts = self.accounts.borrow().clone();
        let mut point = POINT::default();
        let command = unsafe {
            let menu = CreatePopupMenu();
            if menu.is_null() {
                return;
            }
            AppendMenuW(menu, MF_STRING, SHOW, w!("显示窗口"));
            AppendMenuW(menu, MF_STRING, ADD, w!("添加条目"));
            let copy_menu = CreatePopupMenu();
            if !copy_menu.is_null() {
                for (index, (_, name)) in accounts.iter().enumerate() {
                    let title: Vec<u16> = name
                        .replace('&', "&&")
                        .replace('\0', " ")
                        .encode_utf16()
                        .chain([0])
                        .collect();
                    AppendMenuW(copy_menu, MF_STRING, FIRST_ACCOUNT + index, title.as_ptr());
                }
                let flag = if accounts.is_empty() {
                    MF_GRAYED
                } else {
                    MF_ENABLED
                };
                if AppendMenuW(menu, MF_POPUP | flag, copy_menu as usize, w!("复制验证码")) == 0
                {
                    DestroyMenu(copy_menu);
                }
            }
            AppendMenuW(menu, MF_SEPARATOR, 0, null());
            AppendMenuW(menu, MF_STRING, QUIT, w!("退出"));
            GetCursorPos(&mut point);
            SetForegroundWindow(window);
            let choice = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
                point.x,
                point.y,
                0,
                window,
                null(),
            ) as usize;
            PostMessageW(window, WM_NULL, 0, 0);
            DestroyMenu(menu);
            choice
        };
        let action = match command {
            SHOW => TrayAction::Show,
            ADD => TrayAction::Add,
            QUIT => TrayAction::Quit,
            id if id >= FIRST_ACCOUNT => match accounts.get(id - FIRST_ACCOUNT) {
                Some((account, _)) => TrayAction::Copy(account.clone()),
                None => return,
            },
            _ => return,
        };
        let _ = self.sender.send(action);
    }
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // The pointer is set after creation and cleared before destruction. The
    // Box stays at a fixed address and is owned by Tray on this thread.
    let state = unsafe { (GetWindowLongPtrW(window, GWLP_USERDATA) as *const State).as_ref() };
    if let Some(state) = state {
        if message == TRAY_MESSAGE {
            match lparam as u32 {
                WM_LBUTTONUP => {
                    let _ = state.sender.send(TrayAction::Show);
                }
                WM_RBUTTONUP | WM_CONTEXTMENU => state.show_menu(window),
                _ => {}
            }
            return 0;
        }
        if message != 0 && message == state.taskbar_created {
            if !state.register(window) {
                let _ = state.sender.send(TrayAction::Show);
            }
            return 0;
        }
    }
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

impl Drop for Tray {
    fn drop(&mut self) {
        unsafe {
            SetWindowLongPtrW(self.window, GWLP_USERDATA, 0);
            if self.is_available() {
                Shell_NotifyIconW(NIM_DELETE, &self.state.icon_data(self.window));
            }
            DestroyWindow(self.window);
            if !self.state.icon.is_null() {
                DestroyIcon(self.state.icon);
            }
        }
    }
}
