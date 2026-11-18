use std::path::{Path, PathBuf};

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let m = root.join("app.manifest");
    println!("cargo:rerun-if-changed=app.manifest");
    println!("cargo:rerun-if-changed=assets/app.ico");
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}", m.display());
    embed_resources(root);
}

/// Newest x64 rc.exe from the Windows 10/11 SDK (it is not on PATH outside a Developer Prompt).
fn find_rc() -> Option<PathBuf> {
    if let Ok(rc) = std::env::var("RC") {
        return Some(PathBuf::from(rc));
    }
    let bin = Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let mut versions: Vec<PathBuf> = std::fs::read_dir(bin).ok()?.flatten().map(|e| e.path().join(r"x64\rc.exe")).filter(|p| p.exists()).collect();
    versions.sort();
    versions.pop()
}

/// App icon and version info (Explorer's Details tab, Settings > Apps). Skipped with a warning when the SDK is missing.
fn embed_resources(root: &Path) {
    let Some(rc) = find_rc() else {
        println!("cargo:warning=rc.exe not found (install the Windows SDK); building without icon and version info");
        return;
    };
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let ver = env!("CARGO_PKG_VERSION");
    let parts: Vec<&str> = ver.split('.').collect();
    let num = |i: usize| parts.get(i).copied().unwrap_or("0");
    let icon = root.join("assets").join("app.ico").display().to_string().replace('\\', "\\\\");
    let script = format!(
        r#"1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {a},{b},{c},0
PRODUCTVERSION {a},{b},{c},0
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "FileDescription", "Files"
      VALUE "ProductName", "Files"
      VALUE "FileVersion", "{ver}"
      VALUE "ProductVersion", "{ver}"
      VALUE "OriginalFilename", "fexplorer.exe"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
        a = num(0),
        b = num(1),
        c = num(2),
    );
    let rc_file = out.join("app.rc");
    let res_file = out.join("app.res");
    std::fs::write(&rc_file, script).expect("write app.rc");
    let status = std::process::Command::new(&rc).arg("/nologo").arg("/fo").arg(&res_file).arg(&rc_file).status();
    match status {
        Ok(s) if s.success() => println!("cargo:rustc-link-arg-bins={}", res_file.display()),
        other => println!("cargo:warning=rc.exe failed ({other:?}); building without icon and version info"),
    }
}
