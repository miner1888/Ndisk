//! 流式 ZIP 打包(store 方式,恒定内存)。
//!
//! 标准 ZIP 结构:每个条目先写「局部文件头」再写文件数据,最后写「中央目录」
//! 与「中央目录结束记录」。文件名为 UTF-8 并置通用标志位 bit 11(0x0800)。
//! 为保证局部头里的 CRC32 与长度正确,每个源文件读两遍(内存占用不变)。
//! 无 ZIP64:单条目 / 总产物超过 4GB 直接报错(上层另有 2GB 检查)。

use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};

const U32_MAX: u64 = u32::MAX as u64;
const FLAG_UTF8: u16 = 0x0800;
const CRC_TABLE: [u32; 256] = build_crc_table();

const fn build_crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut bit = 0;
        while bit < 8 {
            c = if c & 1 != 0 { 0xEDB88320 ^ (c >> 1) } else { c >> 1 };
            bit += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

fn crc32_update(prev: u32, data: &[u8]) -> u32 {
    let mut c = prev;
    for &b in data {
        c = CRC_TABLE[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    c
}

struct Central {
    name: String,
    crc: u32,
    size: u32,
    offset: u32,
}

/// 把 `(zip 内名称, 磁盘路径)` 列表写成 ZIP 文件,返回最终字节数。
pub fn build_zip_to_file(entries: &[(String, PathBuf)], out_path: &Path) -> Result<u64, String> {
    let mut out = std::fs::File::create(out_path).map_err(|e| format!("创建临时文件失败: {e}"))?;
    let mut buf = vec![0u8; 64 * 1024];
    let mut centrals: Vec<Central> = Vec::new();

    for (name, path) in entries {
        // 第一遍:统计 CRC32 与字节数(局部文件头必须写在文件数据之前)。
        let mut crc = 0xFFFFFFFFu32;
        let mut size: u64 = 0;
        {
            let mut src = std::fs::File::open(path).map_err(|e| format!("读取 {name} 失败: {e}"))?;
            loop {
                let n = src.read(&mut buf).map_err(|e| format!("读取 {name} 失败: {e}"))?;
                if n == 0 {
                    break;
                }
                crc = crc32_update(crc, &buf[..n]);
                size += n as u64;
            }
        }
        let crc = crc ^ 0xFFFFFFFF;
        if size > U32_MAX {
            return Err(format!("{name} 超过 4GB,暂不支持打包"));
        }
        let offset = out.stream_position().map_err(|e| e.to_string())?;
        if offset > U32_MAX {
            return Err("打包结果超过 4GB,暂不支持".into());
        }
        let offset = offset as u32;
        let name_bytes = name.as_bytes();

        // 局部文件头。
        put(&mut out, &[0x50, 0x4B, 0x03, 0x04])?;
        put(&mut out, &20u16.to_le_bytes())?; // 解压所需版本
        put(&mut out, &FLAG_UTF8.to_le_bytes())?; // 通用标志位
        put(&mut out, &0u16.to_le_bytes())?; // 压缩方法:store
        put(&mut out, &0u16.to_le_bytes())?; // 修改时间
        put(&mut out, &0u16.to_le_bytes())?; // 修改日期
        put(&mut out, &crc.to_le_bytes())?;
        put(&mut out, &(size as u32).to_le_bytes())?; // 压缩后大小
        put(&mut out, &(size as u32).to_le_bytes())?; // 原始大小
        put(&mut out, &(name_bytes.len() as u16).to_le_bytes())?;
        put(&mut out, &0u16.to_le_bytes())?; // 扩展字段长度
        put(&mut out, name_bytes)?;

        // 第二遍:写入文件数据(恒定内存)。
        {
            let mut src = std::fs::File::open(path).map_err(|e| format!("读取 {name} 失败: {e}"))?;
            let mut written: u64 = 0;
            loop {
                let n = src.read(&mut buf).map_err(|e| format!("读取 {name} 失败: {e}"))?;
                if n == 0 {
                    break;
                }
                put(&mut out, &buf[..n])?;
                written += n as u64;
            }
            if written != size {
                return Err(format!("{name} 读取字节数与统计不一致(源文件被改动?)"));
            }
        }

        centrals.push(Central { name: name.clone(), crc, size: size as u32, offset });
    }

    // 中央目录。
    let cd_start = out.stream_position().map_err(|e| e.to_string())?;
    if cd_start > U32_MAX {
        return Err("打包结果超过 4GB,暂不支持".into());
    }
    for c in &centrals {
        let name_bytes = c.name.as_bytes();
        put(&mut out, &[0x50, 0x4B, 0x01, 0x02])?; // 中央目录文件头
        put(&mut out, &20u16.to_le_bytes())?; // 创建版本
        put(&mut out, &20u16.to_le_bytes())?; // 解压所需版本
        put(&mut out, &FLAG_UTF8.to_le_bytes())?; // 通用标志位
        put(&mut out, &0u16.to_le_bytes())?; // 压缩方法
        put(&mut out, &0u16.to_le_bytes())?; // 修改时间
        put(&mut out, &0u16.to_le_bytes())?; // 修改日期
        put(&mut out, &c.crc.to_le_bytes())?;
        put(&mut out, &c.size.to_le_bytes())?; // 压缩后大小
        put(&mut out, &c.size.to_le_bytes())?; // 原始大小
        put(&mut out, &(name_bytes.len() as u16).to_le_bytes())?; // 文件名长度
        put(&mut out, &0u16.to_le_bytes())?; // 扩展字段长度
        put(&mut out, &0u16.to_le_bytes())?; // 注释长度
        put(&mut out, &0u16.to_le_bytes())?; // 起始磁盘号
        put(&mut out, &0u16.to_le_bytes())?; // 内部属性
        put(&mut out, &0u32.to_le_bytes())?; // 外部属性
        put(&mut out, &c.offset.to_le_bytes())?; // 局部头偏移
        put(&mut out, name_bytes)?;
    }
    let cd_size = (out.stream_position().map_err(|e| e.to_string())? - cd_start) as u32;

    // 中央目录结束记录。
    put(&mut out, &[0x50, 0x4B, 0x05, 0x06])?;
    put(&mut out, &0u16.to_le_bytes())?; // 当前磁盘号
    put(&mut out, &0u16.to_le_bytes())?; // 中央目录起始磁盘号
    put(&mut out, &(centrals.len() as u16).to_le_bytes())?; // 本磁盘条目数
    put(&mut out, &(centrals.len() as u16).to_le_bytes())?; // 总条目数
    put(&mut out, &cd_size.to_le_bytes())?;
    put(&mut out, &(cd_start as u32).to_le_bytes())?;
    put(&mut out, &0u16.to_le_bytes())?; // 注释长度
    out.flush().map_err(|e| format!("写入打包文件失败: {e}"))?;
    out.stream_position().map_err(|e| e.to_string())
}

fn put(out: &mut std::fs::File, v: &[u8]) -> Result<(), String> {
    out.write_all(v).map_err(|e| format!("写入打包文件失败: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
        hay.windows(needle.len()).position(|w| w == needle)
    }

    #[test]
    fn zip_structure_is_standard() {
        let dir = std::env::temp_dir().join(format!("ndv2_zip_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.txt");
        std::fs::write(&f, b"hello").unwrap();
        let out = dir.join("out.zip");
        let size = build_zip_to_file(&[("a.txt".to_string(), f)], &out).unwrap();
        let bytes = std::fs::read(&out).unwrap();
        assert_eq!(size as usize, bytes.len());

        // 局部文件头在最前,其后是文件名、数据。
        assert_eq!(&bytes[0..4], &[0x50, 0x4B, 0x03, 0x04]);
        let name_len = u16::from_le_bytes([bytes[26], bytes[27]]) as usize;
        assert_eq!(&bytes[30..30 + name_len], b"a.txt");
        assert_eq!(&bytes[30 + name_len..30 + name_len + 5], b"hello");
        // CRC32("hello") = 0x3610a686
        let crc = u32::from_le_bytes([bytes[14], bytes[15], bytes[16], bytes[17]]);
        assert_eq!(crc, 0x3610_a686);

        // 中央目录与结束记录存在;偏移自洽。
        let cd = find(&bytes, &[0x50, 0x4B, 0x01, 0x02]).unwrap();
        let eocd = find(&bytes, &[0x50, 0x4B, 0x05, 0x06]).unwrap();
        assert!(cd < eocd);
        let off = u32::from_le_bytes([bytes[cd + 42], bytes[cd + 43], bytes[cd + 44], bytes[cd + 45]]);
        assert_eq!(off, 0);
        let eocd_cd_off = u32::from_le_bytes([
            bytes[eocd + 16],
            bytes[eocd + 17],
            bytes[eocd + 18],
            bytes[eocd + 19],
        ]);
        assert_eq!(eocd_cd_off as usize, cd);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn crc32_known_value() {
        assert_eq!(crc32_update(0xFFFFFFFF, b"hello") ^ 0xFFFFFFFF, 0x3610_a686);
    }
}
