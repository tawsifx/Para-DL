//! The native host's one Windows call: letting the app come to the front.
#![allow(unsafe_code)]

/// Let process `pid` (the app) bring its windows to the front. Windows refuses that to a program
/// in the background (the foreground lock), so a window opened at the browser's request (Options
/// from the video button, Download File Info) would open behind it. The browser is in front and
/// started this host, which lets the host pass that right on. Nothing happens when it can't.
pub fn allow_foreground(pid: u32) {
    use windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;
    // SAFETY: a plain call with a process id; it fails harmlessly for a wrong or ended one.
    unsafe {
        AllowSetForegroundWindow(pid);
    }
}
