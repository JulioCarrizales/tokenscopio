//! Ícono en la bandeja del sistema (system tray) y notificaciones nativas de Windows.
//!
//! Implementado con FFI puro a `user32`, `shell32` y `kernel32` sin dependencias externas.

use crate::alerts::AlertSeverity;
use eframe::egui;
use std::sync::mpsc::Receiver;
use std::sync::Arc;

/// Acciones disparadas desde el ícono o el menú contextual de la bandeja.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayEvent {
    ShowWindow,
    OpenQuotas,
    ReloadNow,
    Quit,
}

/// Controlador thread-safe del ícono en la bandeja del sistema.
#[derive(Clone)]
pub struct TrayHandle {
    inner: Arc<TrayInner>,
}

impl TrayHandle {
    /// Inicia el hilo dedicado de la bandeja y registra el ícono en Windows.
    pub fn start(ctx: egui::Context) -> Option<(Self, Receiver<TrayEvent>)> {
        #[cfg(target_os = "windows")]
        {
            win32::start_tray(ctx)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = ctx;
            None
        }
    }

    /// Actualiza el tooltip mostrado al pasar el cursor sobre el ícono.
    pub fn update_tooltip(&self, tip: impl Into<String>) {
        self.inner.update_tooltip(tip.into());
    }

    /// Muestra una notificación local (balloon/toast) desde el ícono de la bandeja.
    pub fn notify(&self, title: &str, body: &str, severity: AlertSeverity) {
        self.inner.notify(title, body, severity);
    }

    /// Elimina el ícono de la bandeja y detiene su hilo de mensajes.
    pub fn shutdown(&self) {
        self.inner.shutdown();
    }
}

/// Copia `s` en UTF-16 dentro de `dst`, truncando sin partir pares subrogados y terminando en NUL.
#[cfg(any(target_os = "windows", test))]
fn copy_utf16(dst: &mut [u16], s: &str) {
    dst.fill(0);
    if dst.is_empty() {
        return;
    }
    let max_units = dst.len() - 1;
    let mut pos = 0usize;
    let mut buf = [0u16; 2];
    for ch in s.chars() {
        let encoded = ch.encode_utf16(&mut buf);
        if pos + encoded.len() > max_units {
            break;
        }
        dst[pos..pos + encoded.len()].copy_from_slice(encoded);
        pos += encoded.len();
    }
    dst[pos] = 0;
}

#[cfg(not(target_os = "windows"))]
struct TrayInner;

#[cfg(not(target_os = "windows"))]
impl TrayInner {
    fn update_tooltip(&self, _tip: String) {}
    fn notify(&self, _title: &str, _body: &str, _severity: AlertSeverity) {}
    fn shutdown(&self) {}
}

#[cfg(target_os = "windows")]
use win32::TrayInner;

#[cfg(target_os = "windows")]
mod win32 {
    use super::{copy_utf16, AlertSeverity, TrayEvent, TrayHandle};
    use eframe::egui;
    use std::collections::VecDeque;
    use std::ffi::c_void;
    use std::mem::size_of;
    use std::ptr::null;
    use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::sync::{Arc, Mutex};

    const WM_NULL: u32 = 0x0000;
    const WM_DESTROY: u32 = 0x0002;
    const WM_CLOSE: u32 = 0x0010;
    const WM_CONTEXTMENU: u32 = 0x007B;
    const WM_LBUTTONUP: u32 = 0x0202;
    const WM_LBUTTONDBLCLK: u32 = 0x0203;
    const WM_RBUTTONUP: u32 = 0x0205;
    const WM_APP: u32 = 0x8000;
    const WM_TRAY_ICON: u32 = WM_APP + 1;
    const WM_TRAY_CMD: u32 = WM_APP + 2;

    const GWLP_USERDATA: i32 = -21;

    const NIM_ADD: u32 = 0x0000_0000;
    const NIM_MODIFY: u32 = 0x0000_0001;
    const NIM_DELETE: u32 = 0x0000_0002;

    const NIF_MESSAGE: u32 = 0x0000_0001;
    const NIF_ICON: u32 = 0x0000_0002;
    const NIF_TIP: u32 = 0x0000_0004;
    const NIF_INFO: u32 = 0x0000_0010;

    const NIIF_INFO: u32 = 0x0000_0001;
    const NIIF_WARNING: u32 = 0x0000_0002;
    const NIIF_ERROR: u32 = 0x0000_0003;

    const IDI_APPLICATION: usize = 32512;

    const MF_STRING: u32 = 0x0000_0000;
    const MF_SEPARATOR: u32 = 0x0000_0800;
    const TPM_RIGHTBUTTON: u32 = 0x0002;
    const TPM_NONOTIFY: u32 = 0x0080;
    const TPM_RETURNCMD: u32 = 0x0100;

    const IDM_SHOW: usize = 1001;
    const IDM_QUOTAS: usize = 1002;
    const IDM_RELOAD: usize = 1003;
    const IDM_QUIT: usize = 1004;

    const TRAY_ICON_ID: u32 = 1;
    const CLASS_NAME: &str = "TokenscopioTrayClass";

    type WndProc = unsafe extern "system" fn(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub(super) struct Guid {
        data1: u32,
        data2: u16,
        data3: u16,
        data4: [u8; 8],
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub(super) struct NotifyIconDataW {
        cb_size: u32,
        hwnd: isize,
        u_id: u32,
        u_flags: u32,
        u_callback_message: u32,
        h_icon: isize,
        sz_tip: [u16; 128],
        dw_state: u32,
        dw_state_mask: u32,
        sz_info: [u16; 256],
        u_timeout_or_version: u32,
        sz_info_title: [u16; 64],
        dw_info_flags: u32,
        guid_item: Guid,
        h_balloon_icon: isize,
    }

    impl Default for NotifyIconDataW {
        fn default() -> Self {
            Self {
                cb_size: size_of::<Self>() as u32,
                hwnd: 0,
                u_id: 0,
                u_flags: 0,
                u_callback_message: 0,
                h_icon: 0,
                sz_tip: [0; 128],
                dw_state: 0,
                dw_state_mask: 0,
                sz_info: [0; 256],
                u_timeout_or_version: 0,
                sz_info_title: [0; 64],
                dw_info_flags: 0,
                guid_item: Guid::default(),
                h_balloon_icon: 0,
            }
        }
    }

    #[repr(C)]
    struct WndClassExW {
        cb_size: u32,
        style: u32,
        lpfn_wnd_proc: WndProc,
        cb_cls_extra: i32,
        cb_wnd_extra: i32,
        h_instance: isize,
        h_icon: isize,
        h_cursor: isize,
        hbr_background: isize,
        lpsz_menu_name: *const u16,
        lpsz_class_name: *const u16,
        h_icon_sm: isize,
    }

    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }

    #[repr(C)]
    struct Msg {
        hwnd: isize,
        message: u32,
        wparam: usize,
        lparam: isize,
        time: u32,
        pt: Point,
    }

    #[link(name = "user32")]
    extern "system" {
        fn RegisterClassExW(lpwcx: *const WndClassExW) -> u16;
        fn UnregisterClassW(lp_class_name: *const u16, h_instance: isize) -> i32;
        fn CreateWindowExW(
            dw_ex_style: u32,
            lp_class_name: *const u16,
            lp_window_name: *const u16,
            dw_style: u32,
            x: i32,
            y: i32,
            n_width: i32,
            n_height: i32,
            h_wnd_parent: isize,
            h_menu: isize,
            h_instance: isize,
            lp_param: *mut c_void,
        ) -> isize;
        fn DestroyWindow(h_wnd: isize) -> i32;
        fn DefWindowProcW(h_wnd: isize, msg: u32, w_param: usize, l_param: isize) -> isize;
        fn GetMessageW(lp_msg: *mut Msg, h_wnd: isize, w_msg_filter_min: u32, w_msg_filter_max: u32) -> i32;
        fn TranslateMessage(lp_msg: *const Msg) -> i32;
        fn DispatchMessageW(lp_msg: *const Msg) -> isize;
        fn PostMessageW(h_wnd: isize, msg: u32, w_param: usize, l_param: isize) -> i32;
        fn PostQuitMessage(n_exit_code: i32);
        fn LoadIconW(h_instance: isize, lp_icon_name: *const u16) -> isize;
        fn CreatePopupMenu() -> isize;
        fn AppendMenuW(h_menu: isize, u_flags: u32, u_id_new_item: usize, lp_new_item: *const u16) -> i32;
        fn DestroyMenu(h_menu: isize) -> i32;
        fn GetCursorPos(lp_point: *mut Point) -> i32;
        fn SetForegroundWindow(h_wnd: isize) -> i32;
        fn TrackPopupMenu(
            h_menu: isize,
            u_flags: u32,
            x: i32,
            y: i32,
            n_reserved: i32,
            h_wnd: isize,
            prc_rect: *const c_void,
        ) -> i32;
        #[cfg(target_pointer_width = "64")]
        fn GetWindowLongPtrW(h_wnd: isize, n_index: i32) -> isize;
        #[cfg(target_pointer_width = "64")]
        fn SetWindowLongPtrW(h_wnd: isize, n_index: i32, dw_new_long: isize) -> isize;
        #[cfg(target_pointer_width = "32")]
        fn GetWindowLongW(h_wnd: isize, n_index: i32) -> i32;
        #[cfg(target_pointer_width = "32")]
        fn SetWindowLongW(h_wnd: isize, n_index: i32, dw_new_long: i32) -> i32;
    }

    #[link(name = "shell32")]
    extern "system" {
        fn Shell_NotifyIconW(dw_message: u32, lp_data: *const NotifyIconDataW) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleW(lp_module_name: *const u16) -> isize;
    }

    #[cfg(target_pointer_width = "64")]
    unsafe fn get_user_data(hwnd: isize) -> isize {
        GetWindowLongPtrW(hwnd, GWLP_USERDATA)
    }

    #[cfg(target_pointer_width = "64")]
    unsafe fn set_user_data(hwnd: isize, ptr: isize) {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, ptr);
    }

    #[cfg(target_pointer_width = "32")]
    unsafe fn get_user_data(hwnd: isize) -> isize {
        GetWindowLongW(hwnd, GWLP_USERDATA) as isize
    }

    #[cfg(target_pointer_width = "32")]
    unsafe fn set_user_data(hwnd: isize, ptr: isize) {
        SetWindowLongW(hwnd, GWLP_USERDATA, ptr as i32);
    }

    enum TrayCommand {
        UpdateTooltip(String),
        Notify {
            title: String,
            body: String,
            severity: AlertSeverity,
        },
        Shutdown,
    }

    pub(super) struct TrayInner {
        hwnd: AtomicIsize,
        queue: Mutex<VecDeque<TrayCommand>>,
        closed: AtomicBool,
    }

    impl TrayInner {
        fn new() -> Self {
            Self {
                hwnd: AtomicIsize::new(0),
                queue: Mutex::new(VecDeque::new()),
                closed: AtomicBool::new(false),
            }
        }

        fn post(&self, cmd: TrayCommand) {
            if self.closed.load(Ordering::Acquire) {
                return;
            }
            let hwnd = self.hwnd.load(Ordering::Acquire);
            if hwnd == 0 {
                return;
            }
            if let Ok(mut q) = self.queue.lock() {
                q.push_back(cmd);
            }
            unsafe {
                PostMessageW(hwnd, WM_TRAY_CMD, 0, 0);
            }
        }

        pub(super) fn update_tooltip(&self, tip: String) {
            self.post(TrayCommand::UpdateTooltip(tip));
        }

        pub(super) fn notify(&self, title: &str, body: &str, severity: AlertSeverity) {
            self.post(TrayCommand::Notify {
                title: title.to_string(),
                body: body.to_string(),
                severity,
            });
        }

        pub(super) fn shutdown(&self) {
            self.post(TrayCommand::Shutdown);
        }
    }

    impl Drop for TrayInner {
        fn drop(&mut self) {
            self.shutdown();
        }
    }

    struct ThreadState {
        inner: Arc<TrayInner>,
        tx: Sender<TrayEvent>,
        ctx: egui::Context,
        nid: NotifyIconDataW,
    }

    pub(super) fn start_tray(ctx: egui::Context) -> Option<(TrayHandle, Receiver<TrayEvent>)> {
        let inner = Arc::new(TrayInner::new());
        let (tx, rx) = mpsc::channel();
        let (init_tx, init_rx) = mpsc::sync_channel(1);
        let thread_inner = Arc::clone(&inner);

        let spawned = std::thread::Builder::new()
            .name("tokenscopio-tray".into())
            .spawn(move || run_tray_loop(thread_inner, tx, ctx, init_tx));

        if spawned.is_err() {
            return None;
        }
        match init_rx.recv() {
            Ok(true) => Some((TrayHandle { inner }, rx)),
            _ => None,
        }
    }

    fn run_tray_loop(
        inner: Arc<TrayInner>,
        tx: Sender<TrayEvent>,
        ctx: egui::Context,
        init_tx: mpsc::SyncSender<bool>,
    ) {
        let class_w = to_wide_z(CLASS_NAME);
        let title_w = to_wide_z("Tokenscopio");

        unsafe {
            let h_instance = GetModuleHandleW(null());
            let wc = WndClassExW {
                cb_size: size_of::<WndClassExW>() as u32,
                style: 0,
                lpfn_wnd_proc: wnd_proc,
                cb_cls_extra: 0,
                cb_wnd_extra: 0,
                h_instance,
                h_icon: 0,
                h_cursor: 0,
                hbr_background: 0,
                lpsz_menu_name: null(),
                lpsz_class_name: class_w.as_ptr(),
                h_icon_sm: 0,
            };
            RegisterClassExW(&wc);

            let hwnd = CreateWindowExW(
                0,
                class_w.as_ptr(),
                title_w.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                h_instance,
                std::ptr::null_mut(),
            );
            if hwnd == 0 {
                UnregisterClassW(class_w.as_ptr(), h_instance);
                let _ = init_tx.send(false);
                return;
            }

            let mut h_icon = LoadIconW(h_instance, 1usize as *const u16);
            if h_icon == 0 {
                h_icon = LoadIconW(0, IDI_APPLICATION as *const u16);
            }

            let mut nid = NotifyIconDataW {
                hwnd,
                u_id: TRAY_ICON_ID,
                u_flags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
                u_callback_message: WM_TRAY_ICON,
                h_icon,
                ..Default::default()
            };
            copy_utf16(&mut nid.sz_tip, "Tokenscopio");

            if Shell_NotifyIconW(NIM_ADD, &nid) == 0 {
                DestroyWindow(hwnd);
                UnregisterClassW(class_w.as_ptr(), h_instance);
                let _ = init_tx.send(false);
                return;
            }

            let state_ptr = Box::into_raw(Box::new(ThreadState {
                inner: Arc::clone(&inner),
                tx,
                ctx,
                nid,
            }));
            set_user_data(hwnd, state_ptr as isize);
            inner.hwnd.store(hwnd, Ordering::Release);
            let _ = init_tx.send(true);

            let mut msg = Msg {
                hwnd: 0,
                message: 0,
                wparam: 0,
                lparam: 0,
                time: 0,
                pt: Point { x: 0, y: 0 },
            };
            while GetMessageW(&mut msg, 0, 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            inner.closed.store(true, Ordering::Release);
            inner.hwnd.store(0, Ordering::Release);
            set_user_data(hwnd, 0);
            let state = Box::from_raw(state_ptr);
            Shell_NotifyIconW(NIM_DELETE, &state.nid);
            drop(state);
            UnregisterClassW(class_w.as_ptr(), h_instance);
        }
    }

    unsafe extern "system" fn wnd_proc(
        hwnd: isize,
        msg: u32,
        wparam: usize,
        lparam: isize,
    ) -> isize {
        match msg {
            WM_TRAY_ICON => {
                let Some(state) = thread_state_mut(hwnd) else {
                    return 0;
                };
                let mouse_msg = (lparam as u32) & 0xFFFF;
                match mouse_msg {
                    WM_LBUTTONUP | WM_LBUTTONDBLCLK => {
                        let _ = state.tx.send(TrayEvent::ShowWindow);
                        wake_egui(&state.ctx, true, true);
                    }
                    WM_RBUTTONUP | WM_CONTEXTMENU => {
                        show_context_menu(hwnd, state);
                    }
                    _ => {}
                }
                0
            }
            WM_TRAY_CMD => {
                if let Some(state) = thread_state_mut(hwnd) {
                    drain_commands(hwnd, state);
                }
                0
            }
            WM_CLOSE => {
                DestroyWindow(hwnd);
                0
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    unsafe fn thread_state_mut<'a>(hwnd: isize) -> Option<&'a mut ThreadState> {
        let ptr = get_user_data(hwnd) as *mut ThreadState;
        ptr.as_mut()
    }

    unsafe fn show_context_menu(hwnd: isize, state: &ThreadState) {
        let menu = CreatePopupMenu();
        if menu == 0 {
            return;
        }
        let items = [
            (IDM_SHOW, "Abrir Tokenscopio"),
            (IDM_QUOTAS, "Ver Cuotas"),
            (IDM_RELOAD, "Recargar ahora"),
        ];
        for (id, label) in items {
            let label_w = to_wide_z(label);
            AppendMenuW(menu, MF_STRING, id, label_w.as_ptr());
        }
        AppendMenuW(menu, MF_SEPARATOR, 0, null());
        let quit_w = to_wide_z("Salir");
        AppendMenuW(menu, MF_STRING, IDM_QUIT, quit_w.as_ptr());

        let mut pt = Point { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
            pt.x,
            pt.y,
            0,
            hwnd,
            std::ptr::null(),
        ) as usize;
        PostMessageW(hwnd, WM_NULL, 0, 0);
        DestroyMenu(menu);

        match cmd {
            IDM_SHOW => {
                let _ = state.tx.send(TrayEvent::ShowWindow);
                wake_egui(&state.ctx, true, true);
            }
            IDM_QUOTAS => {
                let _ = state.tx.send(TrayEvent::OpenQuotas);
                wake_egui(&state.ctx, true, true);
            }
            IDM_RELOAD => {
                let _ = state.tx.send(TrayEvent::ReloadNow);
                wake_egui(&state.ctx, false, false);
            }
            IDM_QUIT => {
                let _ = state.tx.send(TrayEvent::Quit);
                wake_egui(&state.ctx, true, false);
            }
            _ => {}
        }
    }

    unsafe fn drain_commands(hwnd: isize, state: &mut ThreadState) {
        let cmds: Vec<TrayCommand> = match state.inner.queue.lock() {
            Ok(mut q) => q.drain(..).collect(),
            Err(_) => return,
        };
        for cmd in cmds {
            match cmd {
                TrayCommand::UpdateTooltip(tip) => {
                    state.nid.u_flags = NIF_TIP;
                    copy_utf16(&mut state.nid.sz_tip, &tip);
                    Shell_NotifyIconW(NIM_MODIFY, &state.nid);
                }
                TrayCommand::Notify {
                    title,
                    body,
                    severity,
                } => {
                    state.nid.u_flags = NIF_INFO;
                    copy_utf16(&mut state.nid.sz_info_title, &title);
                    copy_utf16(&mut state.nid.sz_info, &body);
                    state.nid.dw_info_flags = match severity {
                        AlertSeverity::Info => NIIF_INFO,
                        AlertSeverity::Warning => NIIF_WARNING,
                        AlertSeverity::Critical => NIIF_ERROR,
                    };
                    Shell_NotifyIconW(NIM_MODIFY, &state.nid);
                }
                TrayCommand::Shutdown => {
                    state.inner.closed.store(true, Ordering::Release);
                    Shell_NotifyIconW(NIM_DELETE, &state.nid);
                    DestroyWindow(hwnd);
                    break;
                }
            }
        }
    }

    fn wake_egui(ctx: &egui::Context, show: bool, focus: bool) {
        if show {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        }
        if focus {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        ctx.request_repaint();
    }

    fn to_wide_z(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::copy_utf16;

    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    #[test]
    fn notify_icon_data_size_matches_win32_abi() {
        assert_eq!(std::mem::size_of::<super::win32::NotifyIconDataW>(), 976);
    }

    #[test]
    fn utf16_copy_truncates_and_null_terminates() {
        let mut buf = [0xFFFFu16; 6];
        copy_utf16(&mut buf, "Tokenscopio");
        assert_eq!(buf[5], 0);
        let decoded = String::from_utf16_lossy(&buf[..5]);
        assert_eq!(decoded, "Token");

        // No debe partir un par subrogado si solo queda 1 unidad disponible.
        let mut small = [0xFFFFu16; 4];
        copy_utf16(&mut small, "ab🚀z");
        assert_eq!(&small, &[b'a' as u16, b'b' as u16, 0, 0]);
    }
}
