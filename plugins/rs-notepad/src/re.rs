//! The few RichEdit (msftedit.dll) declarations the notepad needs: windows-sys 0.61 has no RichEdit module.
//! Values and layouts from richedit.h; its structures are declared under `#pragma pack(4)`.
#![allow(non_snake_case, clippy::upper_case_acronyms)]
use windows_sys::Win32::UI::Controls::NMHDR;

const WM_USER: u32 = 0x0400;
pub const EM_EXGETSEL: u32 = WM_USER + 52;
pub const EM_EXLIMITTEXT: u32 = WM_USER + 53;
pub const EM_EXLINEFROMCHAR: u32 = WM_USER + 54;
pub const EM_EXSETSEL: u32 = WM_USER + 55;
pub const EM_SETBKGNDCOLOR: u32 = WM_USER + 67;
pub const EM_SETCHARFORMAT: u32 = WM_USER + 68;
pub const EM_SETEVENTMASK: u32 = WM_USER + 69;
pub const EM_SETTARGETDEVICE: u32 = WM_USER + 72;
pub const EM_STREAMIN: u32 = WM_USER + 73;
pub const EM_SETUNDOLIMIT: u32 = WM_USER + 82;
pub const EM_REDO: u32 = WM_USER + 84;
pub const EM_CANREDO: u32 = WM_USER + 85;
pub const EM_SETTEXTMODE: u32 = WM_USER + 89;
pub const EM_GETTEXTEX: u32 = WM_USER + 94;
pub const EM_GETTEXTLENGTHEX: u32 = WM_USER + 95;
pub const EM_FINDTEXTEXW: u32 = WM_USER + 124;
pub const EM_GETZOOM: u32 = WM_USER + 224;
pub const EM_SETZOOM: u32 = WM_USER + 225;

pub const EN_SELCHANGE: u32 = 0x0702;
pub const EN_DROPFILES: u32 = 0x0703;
pub const ENM_CHANGE: u32 = 0x0001;
pub const ENM_SELCHANGE: u32 = 0x0008_0000;
pub const ENM_DROPFILES: u32 = 0x0010_0000;

pub const SF_TEXT: u32 = 0x0001;
pub const SF_UNICODE: u32 = 0x0010;
pub const SFF_SELECTION: u32 = 0x8000;

pub const TM_PLAINTEXT: u32 = 1;
pub const TM_MULTILEVELUNDO: u32 = 8;
pub const TM_MULTICODEPAGE: u32 = 32;

pub const SCF_DEFAULT: u32 = 0;
pub const SCF_ALL: u32 = 4;
pub const CFM_SIZE: u32 = 0x8000_0000;
pub const CFM_COLOR: u32 = 0x4000_0000;
pub const CFM_FACE: u32 = 0x2000_0000;
pub const CFM_CHARSET: u32 = 0x0800_0000;

pub const GT_DEFAULT: u32 = 0;
pub const GTL_PRECISE: u32 = 2;
pub const GTL_NUMCHARS: u32 = 8;
pub const CP_UNICODE: u32 = 1200;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct CHARRANGE {
    pub cpMin: i32,
    pub cpMax: i32,
}

pub type EditStreamCallback = unsafe extern "system" fn(cookie: usize, buf: *mut u8, cb: i32, pcb: *mut i32) -> u32;

#[repr(C, packed(4))]
pub struct EDITSTREAM {
    pub dwCookie: usize,
    pub dwError: u32,
    pub pfnCallback: Option<EditStreamCallback>,
}

#[repr(C, packed(4))]
pub struct CHARFORMATW {
    pub cbSize: u32,
    pub dwMask: u32,
    pub dwEffects: u32,
    pub yHeight: i32,
    pub yOffset: i32,
    pub crTextColor: u32,
    pub bCharSet: u8,
    pub bPitchAndFamily: u8,
    pub szFaceName: [u16; 32],
}

#[repr(C, packed(4))]
pub struct GETTEXTEX {
    pub cb: u32,
    pub flags: u32,
    pub codepage: u32,
    pub lpDefaultChar: *const u8,
    pub lpUsedDefChar: *mut i32,
}

#[repr(C, packed(4))]
pub struct GETTEXTLENGTHEX {
    pub flags: u32,
    pub codepage: u32,
}

#[repr(C, packed(4))]
pub struct FINDTEXTEXW {
    pub chrg: CHARRANGE,
    pub lpstrText: *const u16,
    pub chrgText: CHARRANGE,
}

#[repr(C, packed(4))]
pub struct ENDROPFILES {
    pub nmhdr: NMHDR,
    pub hDrop: *mut core::ffi::c_void,
    pub cp: i32,
    pub fProtected: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    // sizes from the Windows SDK headers (x64)
    #[test]
    fn layouts_match_richedit_h() {
        assert_eq!(size_of::<EDITSTREAM>(), 20);
        assert_eq!(size_of::<CHARFORMATW>(), 92);
        assert_eq!(size_of::<GETTEXTEX>(), 28);
        assert_eq!(size_of::<FINDTEXTEXW>(), 24);
        assert_eq!(size_of::<ENDROPFILES>(), 40);
    }
}
