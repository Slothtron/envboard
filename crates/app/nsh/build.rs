//! 把 `assets/app.rc`（1 号 ICON）编进 envboard-nsh.exe。
//!
//! NSH 的规则是「exe 自带图标优先」：`nsh-platform-windows` 的 `set_icon()` 先
//! `LoadIconW(instance, 1)`，命中就不再读 `AppOptions.icon` 的 `.ico` 路径。
//! 所以内嵌资源才是正式做法，`icon_path()` 只作开发/无资源时的兜底。
//!
//! 只嵌图标，不嵌 manifest（本 crate 不要求提权、不做兼容层；DPI 由 NSH 运行期设）。
//! `embed-resource` 负责找 `rc.exe`（MSVC）或 `windres`（GNU），
//! 资源文件里的路径相对 rc 所在目录解析。

fn main() {
    println!("cargo::rerun-if-changed=assets/app.rc");
    println!("cargo::rerun-if-changed=assets/envboard.ico");
    embed_resource::compile("assets/app.rc", embed_resource::NONE)
        .manifest_optional()
        .expect("嵌入 Windows 资源失败：检查 rc.exe（MSVC）或 windres（GNU）是否在工具链里");
}
