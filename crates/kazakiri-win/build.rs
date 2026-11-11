//! Windows 用のリソース（アイコン、マニフェスト、バージョン情報）を exe に埋め込む。

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=res");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let ver = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let mut n = ver.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    let (a, b, c) = (n.next().unwrap_or(0), n.next().unwrap_or(0), n.next().unwrap_or(0));
    // バージョンを埋めた .rc を OUT_DIR に作る（アイコンとマニフェストは res/ の絶対パスで指す）
    let res = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("res");
    let path = |name: &str| res.join(name).to_string_lossy().replace('\\', "/");
    let rc = std::fs::read_to_string(res.join("kazakiri.rc"))
        .unwrap()
        .replace("@VERSION_COMMA@", &format!("{a},{b},{c},0"))
        .replace("@VERSION_STR@", &format!("{a}.{b}.{c}.0"))
        .replace("\"kazakiri.ico\"", &format!("\"{}\"", path("kazakiri.ico")))
        .replace("\"kazakiri.manifest\"", &format!("\"{}\"", path("kazakiri.manifest")));
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("kazakiri.rc");
    std::fs::write(&out, rc).unwrap();
    embed_resource::compile(&out, embed_resource::NONE).manifest_required().unwrap();
}
