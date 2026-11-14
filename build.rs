// 构建脚本:仅 Windows 目标下把 assets/ndisk.ico 写进可执行文件资源,
// 让编译出的 ndisk.exe 在资源管理器 / 任务栏里显示自家 logo。
// Linux / macOS 构建时为无操作。

fn main() {
    println!("cargo:rerun-if-changed=assets/ndisk.ico");

    #[cfg(windows)]
    {
        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/ndisk.ico");
        res.set("FileDescription", "NDisk 多用户网盘");
        res.set("ProductName", "NDisk");
        res.set("OriginalFilename", "ndisk.exe");
        match res.compile() {
            Ok(()) => println!("cargo:warning=已嵌入 Windows 图标 assets/ndisk.ico"),
            Err(e) => {
                println!("cargo:warning=嵌入 Windows 图标失败（需要 rc.exe 或 windres）: {e}")
            }
        }
    }
}
