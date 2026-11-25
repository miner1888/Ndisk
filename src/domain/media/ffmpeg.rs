//! ffmpeg 调用封装:CPU 软编参数、命令行构建、错误收敛。

use std::path::Path;

/// CPU 软编参数(libx264 + aac,与历史行为一致,不依赖显卡驱动)。
pub fn cpu_encode_args() -> Vec<String> {
    [
        "-c:v", "libx264", "-preset", "veryfast", "-crf", "23", "-pix_fmt", "yuv420p", "-c:a",
        "aac", "-b:a", "128k",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// 以 `-` 开头的路径会被 ffmpeg 误判为参数,需要前缀 `./`。
pub fn arg(p: &Path) -> String {
    let s = p.to_string_lossy().to_string();
    if s.starts_with('-') {
        format!("./{s}")
    } else {
        s
    }
}

/// 从 stderr 里取最后一行非空内容作为简短错误(ffmpeg 输出很长)。
pub fn last_line(s: &str) -> String {
    s.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string()
}

pub async fn encode_with(
    ffmpeg: &Path,
    input: &Path,
    out: &Path,
    enc: &[String],
) -> Result<(), String> {
    let out_s = out.to_string_lossy().to_string();
    let mut args: Vec<String> = vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-y".into(),
        "-nostdin".into(),
        "-i".into(),
        arg(input),
    ];
    args.extend(enc.iter().cloned());
    args.extend([
        "-movflags".to_string(),
        "+faststart".to_string(),
        "-max_muxing_queue_size".to_string(),
        "2048".to_string(),
        arg(out),
    ]);
    let o = tokio::process::Command::new(ffmpeg)
        .args(&args)
        .output()
        .await
        .map_err(|e| e.to_string())?;
    if o.status.success() && out.is_file() {
        Ok(())
    } else {
        let _ = std::fs::remove_file(&out_s);
        Err(String::from_utf8_lossy(&o.stderr).to_string())
    }
}

pub async fn run_ffmpeg(ffmpeg: &Path, args: &[&str]) -> Result<(), String> {
    let status = tokio::process::Command::new(ffmpeg)
        .args(args)
        .output()
        .await
        .map_err(|e| format!("ffmpeg 启动失败: {e}"))?;
    if status.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&status.stderr).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dash_paths_get_prefix() {
        assert_eq!(arg(Path::new("-weird.mp4")), "./-weird.mp4");
        assert_eq!(arg(Path::new("normal.mp4")), "normal.mp4");
    }

    #[test]
    fn encode_args_order() {
        let a = cpu_encode_args();
        assert_eq!(a[0], "-c:v");
        assert!(a.contains(&"libx264".to_string()));
    }
}
