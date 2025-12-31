use std::ffi::{CStr, CString};

#[repr(C)]
pub struct ctrmml_cmd_string_result {
    pub ok: i32,
    pub message: *mut std::os::raw::c_char,
}

extern "C" {
    pub fn ctrmml_cmd_check_file(path: *const std::os::raw::c_char) -> ctrmml_cmd_string_result;
    pub fn ctrmml_cmd_check_text(
        text: *const std::os::raw::c_char,
        base_dir: *const std::os::raw::c_char,
        display_name: *const std::os::raw::c_char,
    ) -> ctrmml_cmd_string_result;
    pub fn ctrmml_cmd_free_string_result(result: ctrmml_cmd_string_result);
}

pub fn check_file(path: &str) -> Result<(), String> {
    let c_path = CString::new(path).map_err(|_| "path contains null byte".to_string())?;
    let result = unsafe { ctrmml_cmd_check_file(c_path.as_ptr()) };
    convert_result(result)
}

pub fn check_text(text: &str, base_dir: &str, display_name: &str) -> Result<(), String> {
    let c_text = CString::new(text).map_err(|_| "text contains null byte".to_string())?;
    let c_base = CString::new(base_dir).map_err(|_| "base_dir contains null byte".to_string())?;
    let c_name = CString::new(display_name).map_err(|_| "display_name contains null byte".to_string())?;
    let result = unsafe { ctrmml_cmd_check_text(c_text.as_ptr(), c_base.as_ptr(), c_name.as_ptr()) };
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
