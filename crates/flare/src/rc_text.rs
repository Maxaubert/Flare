//! The exe's icon and version resource, as resource-script text (M9 Task 26). Pure: no dependency.
//! `build.rs` writes the text into OUT_DIR and compiles it; the integration test in
//! `tests/ship_text.rs` includes this file by path and checks it.

/// Resource script text for `version` (the workspace version) and `icon_path` (the absolute path of
/// `build/icon.ico`). The numbers come from the first three parts of `version`; the strings keep the
/// whole text, so a pre-release reads as itself.
pub fn rc_text(version: &str, icon_path: &str) -> String {
    let [a, b, c] = numbers(version);
    let icon = icon_path.replace('\\', "\\\\");
    format!(
        r#"1 ICON "{icon}"
#include <winver.h>
VS_VERSION_INFO VERSIONINFO
FILEVERSION {a},{b},{c},0
PRODUCTVERSION {a},{b},{c},0
FILEFLAGSMASK 0x3fL
FILEOS 0x40004L
FILETYPE 0x1L
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904b0"
        BEGIN
            VALUE "CompanyName", "Max"
            VALUE "FileDescription", "Flare"
            VALUE "FileVersion", "{version}.0"
            VALUE "InternalName", "Flare"
            VALUE "LegalCopyright", "Copyright (c) Max"
            VALUE "OriginalFilename", "Flare.exe"
            VALUE "ProductName", "Flare"
            VALUE "ProductVersion", "{version}"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x409, 1200
    END
END
"#
    )
}

/// The `major.minor.patch` numbers of `version`; a part that is missing or not a number is 0.
/// A pre-release suffix (`-beta.1`) is not a number and is cut off first.
fn numbers(version: &str) -> [u16; 3] {
    let core = version.split(['-', '+']).next().unwrap_or("");
    let mut out = [0u16; 3];
    for (slot, part) in out.iter_mut().zip(core.split('.')) {
        *slot = part.parse().unwrap_or(0);
    }
    out
}
