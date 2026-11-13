//! Platform glue for the OSD window.

/// On Linux, prefer XWayland when an X display exists: native Wayland lets
/// neither apps position their own windows nor keep them on top or out of the
/// taskbar, which is the whole point of an OSD. Same trick as the reference
/// widget's `QT_QPA_PLATFORM=xcb`. Must run before any threads are spawned.
pub fn prefer_x11(enabled: bool) {
    #[cfg(target_os = "linux")]
    if enabled
        && std::env::var_os("WAYLAND_DISPLAY").is_some()
        && std::env::var_os("DISPLAY").is_some_and(|d| !d.is_empty())
    {
        // SAFETY: called from main() before any other thread exists.
        unsafe { std::env::remove_var("WAYLAND_DISPLAY") };
    }
    #[cfg(not(target_os = "linux"))]
    let _ = enabled;
}

/// Whether the taskbar setting can be applied live (otherwise needs a relaunch).
pub const TASKBAR_LIVE_TOGGLE: bool = cfg!(target_os = "linux");

type DynResult = Result<(), Box<dyn std::error::Error>>;

/// Add or remove EWMH `_NET_WM_STATE` flags on a *mapped* X11 window.
/// (Window managers ignore these requests before the window is mapped, which
/// is why winit's creation-time always-on-top doesn't stick under KWin.)
/// No-op on non-X11 windows.
#[cfg(target_os = "linux")]
fn set_wm_state(
    window: &impl raw_window_handle::HasWindowHandle,
    enable: bool,
    names: [&[u8]; 2],
) -> DynResult {
    use raw_window_handle::RawWindowHandle;
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{ClientMessageEvent, ConnectionExt, EventMask};

    let xid: u32 = match window.window_handle()?.as_raw() {
        RawWindowHandle::Xlib(h) => h.window as u32,
        RawWindowHandle::Xcb(h) => h.window.get(),
        _ => return Ok(()), // Wayland etc.: the compositor decides.
    };
    let (conn, screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen].root;
    let atom = |name: &[u8]| -> Result<u32, Box<dyn std::error::Error>> {
        Ok(conn.intern_atom(false, name)?.reply()?.atom)
    };
    let wm_state = atom(b"_NET_WM_STATE")?;
    let a = atom(names[0])?;
    let b = if names[1].is_empty() {
        0
    } else {
        atom(names[1])?
    };
    // data: [action (0 remove / 1 add), prop1, prop2, source (1 = application), 0]
    let event = ClientMessageEvent::new(32, xid, wm_state, [u32::from(enable), a, b, 1, 0]);
    conn.send_event(
        false,
        root,
        EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
        event,
    )?
    .check()?; // round-trip before dropping this short-lived X11 connection
    conn.flush()?;
    Ok(())
}

/// Show or hide the taskbar/pager entry (`_NET_WM_STATE_SKIP_TASKBAR`).
#[cfg(target_os = "linux")]
pub fn set_skip_taskbar(window: &impl raw_window_handle::HasWindowHandle, skip: bool) -> DynResult {
    set_wm_state(
        window,
        skip,
        [b"_NET_WM_STATE_SKIP_TASKBAR", b"_NET_WM_STATE_SKIP_PAGER"],
    )
}

/// Keep the window above others (`_NET_WM_STATE_ABOVE`).
#[cfg(target_os = "linux")]
pub fn set_always_on_top(
    window: &impl raw_window_handle::HasWindowHandle,
    on_top: bool,
) -> DynResult {
    set_wm_state(window, on_top, [b"_NET_WM_STATE_ABOVE", b""])
}

#[cfg(not(target_os = "linux"))]
pub fn set_skip_taskbar(
    _window: &impl raw_window_handle::HasWindowHandle,
    _skip: bool,
) -> DynResult {
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn set_always_on_top(
    _window: &impl raw_window_handle::HasWindowHandle,
    _on_top: bool,
) -> DynResult {
    Ok(())
}

/// Start a fresh copy of ourselves (used when a setting needs a new window).
pub fn relaunch() {
    if let Ok(exe) = std::env::current_exe() {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let _ = std::process::Command::new(exe).args(args).spawn();
    }
}
