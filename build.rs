// 构建脚本:仅 Windows 目标下把 assets/ndisk.ico 写进可执行文件资源。
// 其它平台为无操作。

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
            // 图标嵌入是常驻行为,正常路径不向构建日志输出警告;
            // 仅失败(缺 rc.exe / windres)时保留 warning 提醒。
            Ok(()) => {}
            Err(e) => println!("cargo:warning=嵌入 Windows 图标失败(需要 rc.exe 或 windres): {e}"),
        }
    }
}
