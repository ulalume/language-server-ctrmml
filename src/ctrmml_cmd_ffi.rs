use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_void};

#[repr(C)]
pub struct ctrmml_cmd_string_result {
    pub ok: i32,
    pub message: *mut c_char,
}

#[repr(C)]
pub struct ctrmml_cmd_highlight_position {
    pub line: u32,
    pub col: u32,
}

pub type ctrmml_cmd_highlight_callback =
    Option<unsafe extern "C" fn(u32, *const ctrmml_cmd_highlight_position, usize, *mut c_void)>;

#[repr(C)]
pub struct ctrmml_cmd_stop_flag {
    _private: [u8; 0],
}

#[repr(C)]
pub struct ctrmml_cmd_play_options {
    pub follow: i32,
    pub log_messages: i32,
    pub has_start: i32,
    pub start_line: u32,
    pub start_col: u32,
    pub stop_flag: *mut ctrmml_cmd_stop_flag,
    pub on_highlight: ctrmml_cmd_highlight_callback,
    pub user_data: *mut c_void,
}

extern "C" {
    pub fn ctrmml_cmd_check_file(path: *const c_char) -> ctrmml_cmd_string_result;
    pub fn ctrmml_cmd_check_text(
        text: *const c_char,
        base_dir: *const c_char,
        display_name: *const c_char,
    ) -> ctrmml_cmd_string_result;
    pub fn ctrmml_cmd_free_string_result(result: ctrmml_cmd_string_result);

    pub fn ctrmml_cmd_export_vgm_file(
        path: *const c_char,
        out_path: *const c_char,
    ) -> ctrmml_cmd_string_result;
    pub fn ctrmml_cmd_export_vgm_text(
        text: *const c_char,
        base_dir: *const c_char,
        display_name: *const c_char,
        out_path: *const c_char,
    ) -> ctrmml_cmd_string_result;
    pub fn ctrmml_cmd_export_wav_file(
        path: *const c_char,
        out_path: *const c_char,
    ) -> ctrmml_cmd_string_result;
    pub fn ctrmml_cmd_export_wav_text(
        text: *const c_char,
        base_dir: *const c_char,
        display_name: *const c_char,
        out_path: *const c_char,
    ) -> ctrmml_cmd_string_result;

    pub fn ctrmml_cmd_stop_flag_new() -> *mut ctrmml_cmd_stop_flag;
    pub fn ctrmml_cmd_stop_flag_set(flag: *mut ctrmml_cmd_stop_flag);
    pub fn ctrmml_cmd_stop_flag_free(flag: *mut ctrmml_cmd_stop_flag);

    pub fn ctrmml_cmd_play_file(
        path: *const c_char,
        options: *const ctrmml_cmd_play_options,
    ) -> ctrmml_cmd_string_result;
    pub fn ctrmml_cmd_play_text(
        text: *const c_char,
        base_dir: *const c_char,
        display_name: *const c_char,
        options: *const ctrmml_cmd_play_options,
    ) -> ctrmml_cmd_string_result;
}

pub fn check_file(path: &str) -> Result<(), String> {
    let c_path = CString::new(path).map_err(|_| "path contains null byte".to_string())?;
    let result = unsafe { ctrmml_cmd_check_file(c_path.as_ptr()) };
    convert_result(result)
}

pub fn check_text(text: &str, base_dir: &str, display_name: &str) -> Result<(), String> {
    let c_text = CString::new(text).map_err(|_| "text contains null byte".to_string())?;
    let c_base = CString::new(base_dir).map_err(|_| "base_dir contains null byte".to_string())?;
    let c_name =
        CString::new(display_name).map_err(|_| "display_name contains null byte".to_string())?;
    let result = unsafe { ctrmml_cmd_check_text(c_text.as_ptr(), c_base.as_ptr(), c_name.as_ptr()) };
    convert_result(result)
}

pub fn export_vgm_file(path: &str, out_path: &str) -> Result<(), String> {
    let c_path = CString::new(path).map_err(|_| "path contains null byte".to_string())?;
    let c_out = CString::new(out_path).map_err(|_| "out_path contains null byte".to_string())?;
    let result = unsafe { ctrmml_cmd_export_vgm_file(c_path.as_ptr(), c_out.as_ptr()) };
    convert_result(result)
}

pub fn export_vgm_text(
    text: &str,
    base_dir: &str,
    display_name: &str,
    out_path: &str,
) -> Result<(), String> {
    let c_text = CString::new(text).map_err(|_| "text contains null byte".to_string())?;
    let c_base = CString::new(base_dir).map_err(|_| "base_dir contains null byte".to_string())?;
    let c_name =
        CString::new(display_name).map_err(|_| "display_name contains null byte".to_string())?;
    let c_out = CString::new(out_path).map_err(|_| "out_path contains null byte".to_string())?;
    let result = unsafe {
        ctrmml_cmd_export_vgm_text(c_text.as_ptr(), c_base.as_ptr(), c_name.as_ptr(), c_out.as_ptr())
    };
    convert_result(result)
}

pub fn export_wav_file(path: &str, out_path: &str) -> Result<(), String> {
    let c_path = CString::new(path).map_err(|_| "path contains null byte".to_string())?;
    let c_out = CString::new(out_path).map_err(|_| "out_path contains null byte".to_string())?;
    let result = unsafe { ctrmml_cmd_export_wav_file(c_path.as_ptr(), c_out.as_ptr()) };
    convert_result(result)
}

pub fn export_wav_text(
    text: &str,
    base_dir: &str,
    display_name: &str,
    out_path: &str,
) -> Result<(), String> {
    let c_text = CString::new(text).map_err(|_| "text contains null byte".to_string())?;
    let c_base = CString::new(base_dir).map_err(|_| "base_dir contains null byte".to_string())?;
    let c_name =
        CString::new(display_name).map_err(|_| "display_name contains null byte".to_string())?;
    let c_out = CString::new(out_path).map_err(|_| "out_path contains null byte".to_string())?;
    let result = unsafe {
        ctrmml_cmd_export_wav_text(c_text.as_ptr(), c_base.as_ptr(), c_name.as_ptr(), c_out.as_ptr())
    };
    convert_result(result)
}

pub struct StopFlag {
    ptr: *mut ctrmml_cmd_stop_flag,
}

unsafe impl Send for StopFlag {}
unsafe impl Sync for StopFlag {}

impl StopFlag {
    pub fn new() -> Result<Self, String> {
        let ptr = unsafe { ctrmml_cmd_stop_flag_new() };
        if ptr.is_null() {
            return Err("failed to create stop flag".to_string());
        }
        Ok(Self { ptr })
    }

    pub fn set(&self) {
        unsafe { ctrmml_cmd_stop_flag_set(self.ptr) };
    }

    pub fn as_ptr(&self) -> *mut ctrmml_cmd_stop_flag {
        self.ptr
    }
}

impl Drop for StopFlag {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe { ctrmml_cmd_stop_flag_free(self.ptr) };
        }
    }
}

pub fn play_file(path: &str, options: &ctrmml_cmd_play_options) -> Result<(), String> {
    let c_path = CString::new(path).map_err(|_| "path contains null byte".to_string())?;
    let result = unsafe { ctrmml_cmd_play_file(c_path.as_ptr(), options as *const _) };
    convert_result(result)
}

pub fn play_text(
    text: &str,
    base_dir: &str,
    display_name: &str,
    options: &ctrmml_cmd_play_options,
) -> Result<(), String> {
    let c_text = CString::new(text).map_err(|_| "text contains null byte".to_string())?;
    let c_base = CString::new(base_dir).map_err(|_| "base_dir contains null byte".to_string())?;
    let c_name =
        CString::new(display_name).map_err(|_| "display_name contains null byte".to_string())?;
    let result = unsafe {
        ctrmml_cmd_play_text(
            c_text.as_ptr(),
            c_base.as_ptr(),
            c_name.as_ptr(),
            options as *const _,
        )
    };
    convert_result(result)
}

fn convert_result(result: ctrmml_cmd_string_result) -> Result<(), String> {
    let ok = result.ok;
    let message = if result.message.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(result.message) }
            .to_string_lossy()
            .to_string()
    };

    unsafe { ctrmml_cmd_free_string_result(result) };

    if ok != 0 {
        Ok(())
    } else {
        Err(message)
    }
}
