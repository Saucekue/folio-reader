use std::{io, mem::size_of};

use windows_sys::Win32::{
    Foundation::HWND,
    Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow},
    System::LibraryLoader::GetModuleHandleW,
    UI::WindowsAndMessaging::{
        GWL_STYLE, GetSystemMetrics, GetWindowLongW, GetWindowPlacement, HWND_TOP, ICON_BIG,
        ICON_SMALL, IMAGE_ICON, LR_SHARED, LoadImageW, SM_CXICON, SM_CXSMICON, SM_CYICON,
        SM_CYSMICON, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
        SendMessageW, SetWindowLongW, SetWindowPlacement, SetWindowPos, WINDOWPLACEMENT, WM_SETICON,
        WS_OVERLAPPEDWINDOW,
    },
};

use crate::book::Result;

pub fn set_icon(window: HWND) {
    unsafe {
        let module = GetModuleHandleW(std::ptr::null());
        for (kind, width, height) in [(ICON_BIG, SM_CXICON, SM_CYICON), (ICON_SMALL, SM_CXSMICON, SM_CYSMICON)] {
            let icon = LoadImageW(module, 101usize as *const u16, IMAGE_ICON, GetSystemMetrics(width),
                GetSystemMetrics(height), LR_SHARED);
            if !icon.is_null() {
                SendMessageW(window, WM_SETICON, kind as usize, icon as isize);
            }
        }
    }
}

#[derive(Default)]
pub struct Fullscreen {
    previous: Option<(i32, WINDOWPLACEMENT)>,
}

impl Fullscreen {
    pub fn set(&mut self, window: HWND, enabled: bool) -> Result<()> {
        if enabled == self.previous.is_some() { return Ok(()); }
        if let Some((style, placement)) = self.previous.as_ref() {
            unsafe {
                SetWindowLongW(window, GWL_STYLE, *style);
                checked(SetWindowPlacement(window, placement))?;
                checked(SetWindowPos(window, std::ptr::null_mut(), 0, 0, 0, 0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED))?;
            }
            self.previous = None;
        } else {
            unsafe {
                let mut placement = WINDOWPLACEMENT { length: size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
                checked(GetWindowPlacement(window, &mut placement))?;
                let mut monitor = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
                checked(GetMonitorInfoW(MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST), &mut monitor))?;
                let style = GetWindowLongW(window, GWL_STYLE);
                SetWindowLongW(window, GWL_STYLE, style & !(WS_OVERLAPPEDWINDOW as i32));
                let rect = monitor.rcMonitor;
                if let Err(error) = checked(SetWindowPos(window, HWND_TOP, rect.left, rect.top,
                    rect.right - rect.left, rect.bottom - rect.top, SWP_FRAMECHANGED | SWP_NOACTIVATE))
                {
                    SetWindowLongW(window, GWL_STYLE, style);
                    SetWindowPlacement(window, &placement);
                    return Err(error);
                }
                self.previous = Some((style, placement));
            }
        }
        Ok(())
    }
}

fn checked(result: i32) -> Result<()> {
    if result == 0 { return Err(io::Error::last_os_error().into()); }
    Ok(())
}
