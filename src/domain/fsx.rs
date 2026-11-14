use std::path::{Path, PathBuf};

/// 同目录临时文件 + 原子 rename,避免写坏原文件;临时名带进程号 / 时间戳 / 随机数防并发冲突。
pub async fn atomic_write(full: &Path, data: &[u8]) -> Result<(), String> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let dir = full.parent().ok_or_else(|| "非法路径".to_string())?;
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros())
        .unwrap_or(0);
    let rnd = rand::random::<u32>();
    let tmp = dir.join(format!(".nd-save-{}-{ts}-{rnd:x}.tmp", std::process::id()));
    let write = async {
        tokio::fs::write(&tmp, data).await?;
        tokio::fs::rename(&tmp, full).await
    };
    if let Err(e) = write.await {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("保存失败: {e}"));
    }
    Ok(())
}

/// 递归统计目录:总大小 / 文件数 / 子文件夹数(跳过隐藏项与符号链接,深度兜底)。
pub fn stat_tree(dir: &Path, depth: u32, total: &mut u64, files: &mut u64, dirs: &mut u64) {
    if depth > 64 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for de in rd.flatten() {
        let name = de.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(de.path()) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            *dirs += 1;
            stat_tree(&de.path(), depth + 1, total, files, dirs);
        } else if meta.is_file() {
            *files += 1;
            *total += meta.len();
        }
    }
}

/// 递归收集目录下的文件(相对路径,物理路径);不跟随目录符号链接,避免成环。
pub fn collect_files(dir: &Path, rel: &str, out: &mut Vec<(String, PathBuf)>, depth: u32) {
    if depth > 64 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for de in rd.flatten() {
        let name = de.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(de.path()) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        let child_rel = if rel.is_empty() {
            name
        } else {
            format!("{rel}/{name}")
        };
        if meta.is_dir() {
            collect_files(&de.path(), &child_rel, out, depth + 1);
        } else if meta.is_file() {
            out.push((child_rel, de.path()));
        }
    }
}

pub fn unix_secs(t: std::io::Result<std::time::SystemTime>) -> i64 {
    t.ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 无依赖 CRC32(IEEE),供 ZIP 打包使用。
/// 流式 CRC32:初始传 0xFFFFFFFF,分块更新,结束时再异或 0xFFFFFFFF。
fn crc32_update(prev: u32, data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, slot) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB88320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
        *slot = c;
    }
    let mut c = prev;
    for &b in data {
        c = table[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    c
}

/// 简易 ZIP 打包(STORE 存储,不二次压缩媒体等已压缩内容),一次生成完整字节。
/// 流式打包 zip 到临时文件:逐文件按 64KB 块读取 + 边读边算 CRC,
/// 内存占用恒定(与打包内容总体积无关)。返回临时文件大小(字节)。
pub fn build_zip_to_file(entries: &[(String, PathBuf)], out_path: &Path) -> Result<u64, String> {
    struct Central {
        name: String,
        crc: u32,
        size: u32,
        offset: u32,
    }
    use std::io::{Seek, Write};
    let mut out = std::fs::File::create(out_path).map_err(|e| format!("创建临时文件失败: {e}"))?;
    let mut buf = vec![0u8; 64 * 1024];
    let mut centrals: Vec<Central> = Vec::new();
    let put = |out: &mut std::fs::File, v: &[u8]| {
        out.write_all(v)
            .map_err(|e| format!("写入打包文件失败: {e}"))
    };
    for (name, path) in entries {
        let mut src = std::fs::File::open(path).map_err(|e| format!("读取 {name} 失败: {e}"))?;
        let mut crc = 0xFFFFFFFFu32;
        let mut size: u64 = 0;
        loop {
            let n = std::io::Read::read(&mut src, &mut buf)
                .map_err(|e| format!("读取 {name} 失败: {e}"))?;
            if n == 0 {
                break;
            }
            crc = crc32_update(crc, &buf[..n]);
            out.write_all(&buf[..n])
                .map_err(|e| format!("写入打包文件失败: {e}"))?;
            size += n as u64;
        }
        let crc = crc ^ 0xFFFFFFFF;
        let offset = (out.stream_position().map_err(|e| e.to_string())? - size) as u32;
        let name_bytes = name.as_bytes();
        put(&mut out, &[0x50, 0x4B, 0x03, 0x04])?; // local file header
        put(&mut out, &20u16.to_le_bytes())?; // version
        put(&mut out, &0u16.to_le_bytes())?; // flags (UTF-8 names assumed)
        put(&mut out, &0u16.to_le_bytes())?; // method: store
        put(&mut out, &0u16.to_le_bytes())?; // time
        put(&mut out, &0u16.to_le_bytes())?; // date
        put(&mut out, &crc.to_le_bytes())?;
        put(&mut out, &(size as u32).to_le_bytes())?;
        put(&mut out, &(size as u32).to_le_bytes())?;
        put(&mut out, &(name_bytes.len() as u16).to_le_bytes())?;
        put(&mut out, &0u16.to_le_bytes())?; // extra len
        put(&mut out, name_bytes)?;
        centrals.push(Central {
            name: name.clone(),
            crc,
            size: size as u32,
            offset,
        });
    }
    let cd_start = out.stream_position().map_err(|e| e.to_string())? as u32;
    for c in &centrals {
        let name_bytes = c.name.as_bytes();
        put(&mut out, &[0x50, 0x4B, 0x01, 0x02])?; // central dir header
        put(&mut out, &20u16.to_le_bytes())?;
        put(&mut out, &20u16.to_le_bytes())?;
        put(&mut out, &0u16.to_le_bytes())?;
        put(&mut out, &0u16.to_le_bytes())?;
        put(&mut out, &0u16.to_le_bytes())?;
        put(&mut out, &0u16.to_le_bytes())?;
        put(&mut out, &c.crc.to_le_bytes())?;
        put(&mut out, &c.size.to_le_bytes())?;
        put(&mut out, &(name_bytes.len() as u16).to_le_bytes())?;
        put(&mut out, &0u16.to_le_bytes())?; // extra
        put(&mut out, &0u16.to_le_bytes())?; // comment
        put(&mut out, &0u16.to_le_bytes())?; // disk
        put(&mut out, &0u16.to_le_bytes())?; // internal attrs
        put(&mut out, &0u32.to_le_bytes())?; // external attrs
        put(&mut out, &c.offset.to_le_bytes())?;
        put(&mut out, name_bytes)?;
    }
    let cd_size = (out.stream_position().map_err(|e| e.to_string())? - cd_start as u64) as u32;
    put(&mut out, &[0x50, 0x4B, 0x05, 0x06])?; // end of central dir
    put(&mut out, &0u16.to_le_bytes())?;
    put(&mut out, &0u16.to_le_bytes())?;
    put(&mut out, &(centrals.len() as u16).to_le_bytes())?;
    put(&mut out, &(centrals.len() as u16).to_le_bytes())?;
    put(&mut out, &cd_size.to_le_bytes())?;
    put(&mut out, &(cd_start).to_le_bytes())?;
    put(&mut out, &0u16.to_le_bytes())?;
    out.flush().map_err(|e| format!("写入打包文件失败: {e}"))?;
    out.stream_position().map_err(|e| e.to_string())
}
