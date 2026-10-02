// Ресурсы Windows: иконка + манифест (DPI-awareness, поддержка ОС).
// Собирается windres, который идёт вместе с тулчейном GNU.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    let out = std::env::var("OUT_DIR").expect("OUT_DIR");
    let res = PathBuf::from(&out).join("tunnelstat.res");
    let _ = &out;

    // Ищем windres рядом с rustc/cargo или в mingw из PATH.
    let windres = find_windres().unwrap_or_else(|| {
        panic!("windres не найден: поставь тулчейн (scoop install mingw) или добавь его в PATH")
    });

    // ВАЖНО: путь к .rc даём относительным. windres зовёт препроцессор cc1,
    // который не экранирует пробелы, и абсолютный путь вида
    // "<path>\tunnelstat.rc" распадается на аргументы.
    // Текущий каталог build-скрипта — это корень пакета, поэтому хватит имени файла.
    let rc = "tunnelstat.rc";
    let mut cmd = Command::new(&windres);
    cmd.current_dir(std::env::current_dir().unwrap());
    cmd.arg(rc)
        .arg("-O")
        .arg("coff")
        .arg("-o")
        .arg(&res)
        .arg("-I")
        .arg(".");
    let st = cmd.output().expect("не удалось запустить windres");
    // Передаём реальный вывод windres: без него причина падения не видна.
    if !st.stdout.is_empty() {
        print!("{}", String::from_utf8_lossy(&st.stdout));
    }
    if !st.stderr.is_empty() {
        print!("{}", String::from_utf8_lossy(&st.stderr));
    }
    assert!(st.status.success(), "windres завершился с {}", st.status);
    assert!(res.exists(), "windres не создал {}", res.display());

    println!("cargo:rustc-link-arg={}", res.display());
    println!("cargo:rerun-if-changed=tunnelstat.rc");
    println!("cargo:rerun-if-changed=tunnelstat.ico");
    println!("cargo:rerun-if-changed=tunnelstat.manifest");
}

fn find_windres() -> Option<PathBuf> {
    // 1) сам toolchain-каталог, где лежит cargo
    if let Ok(exe) = std::env::current_exe() {
        let mut p = exe.clone();
        p.pop();
        let cand = p.join("windres.exe");
        if cand.exists() {
            return Some(cand);
        }
        // 2) подняться на уровень выше: .../bin/ -> .../
        p.pop();
        let cand = p.join("bin").join("windres.exe");
        if cand.exists() {
            return Some(cand);
        }
    }
    // 3) обычный PATH
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let cand = dir.join("windres.exe");
        if cand.exists() {
            return Some(cand);
        }
    }
    None
}
