//! Process-level setup every MKLM executable does first thing in `main`, and text encoding for a
//! redirected standard output. Nothing here touches the system outside the calling process.

use windows::Win32::Globalization::{CP_UTF8, WideCharToMultiByte};
use windows::Win32::System::Console::GetConsoleOutputCP;
use windows::Win32::System::LibraryLoader::{
    LOAD_LIBRARY_SEARCH_SYSTEM32, SetDefaultDllDirectories,
};
use windows::core::PCSTR;

use crate::error::{Error, win32_code};

/// Makes DLLs loaded at run time come from System32 only (plan 2.2). Static imports are covered
/// by linking with `/DEPENDENTLOADFLAG:0x800`.
pub fn restrict_dll_search() -> Result<(), Error> {
    // SAFETY: the call takes flags only and changes nothing but this process's DLL search order.
    unsafe { SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32) }.map_err(|error| {
        Error::Win32 {
            function: "SetDefaultDllDirectories",
            code: win32_code(&error),
        }
    })
}

/// Encodes `text` for a standard output that is redirected to a pipe or a file.
///
/// PowerShell and cmd decode a native program's output with the console's output code page, so
/// the text is converted to that code page; characters it lacks become `?`. Without a console, or
/// when the code page is UTF-8 already, the text stays UTF-8.
pub fn encode_for_redirected_output(text: &str) -> Vec<u8> {
    // SAFETY: GetConsoleOutputCP has no preconditions; it returns 0 without a console.
    let code_page = unsafe { GetConsoleOutputCP() };
    if code_page == 0 || code_page == CP_UTF8 || text.is_ascii() {
        return text.as_bytes().to_vec();
    }
    encode(text, code_page).unwrap_or_else(|| text.as_bytes().to_vec())
}

/// `text` in `code_page`, or `None` when Windows cannot convert it.
fn encode(text: &str, code_page: u32) -> Option<Vec<u8>> {
    let wide: Vec<u16> = text.encode_utf16().collect();
    // SAFETY: `wide` is valid UTF-16; without an output buffer the call only returns the size.
    let size = unsafe { WideCharToMultiByte(code_page, 0, &wide, None, PCSTR::null(), None) };
    let mut bytes = vec![0u8; usize::try_from(size).ok().filter(|&size| size > 0)?];
    // SAFETY: `bytes` has room for the size the first call reported; the slice length says so.
    let written =
        unsafe { WideCharToMultiByte(code_page, 0, &wide, Some(&mut bytes), PCSTR::null(), None) };
    bytes.truncate(
        usize::try_from(written)
            .ok()
            .filter(|&written| written > 0)?,
    );
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_japanese_in_shift_jis() {
        // 日本語 in code page 932.
        assert_eq!(
            encode("日本語 PS/2", 932).as_deref(),
            Some(&b"\x93\xfa\x96{\x8c\xea PS/2"[..])
        );
        assert_eq!(encode("abc", CP_UTF8).as_deref(), Some(&b"abc"[..]));
        // Characters the code page lacks become one `?` per UTF-16 unit.
        assert_eq!(encode("😀", 932).as_deref(), Some(&b"??"[..]));
    }

    #[test]
    fn dll_search_can_be_restricted() {
        assert_eq!(restrict_dll_search(), Ok(()));
    }
}
