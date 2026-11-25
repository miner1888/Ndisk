//! eD2K 链接与协议层:链接解析、MD4、server.met 订阅、服务器登录与查源、
//! KAD 并行查源聚合、块级下载调度(worker)。

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;

use crate::app::{now_secs, App};
use crate::domain::cached;

pub const ED2K_SERVERS_KEY: &str = "offline_ed2k_servers";
pub const ED2K_CONCURRENCY_KEY: &str = "offline_ed2k_concurrency";
pub const DEFAULT_ED2K_CONCURRENCY: usize = 6;

/// 内置默认服务器列表(ip、port)。
const DEFAULT_SERVER_LIST: [(&str, [u8; 4], u16); 3] = [
    ("TVU", [176, 103, 48, 36], 4184),
    ("PeerBooter", [217, 172, 179, 70], 4232),
    ("PeerBooter2", [80, 208, 228, 241], 4232),
];

pub fn default_servers() -> String {
    DEFAULT_SERVER_LIST
        .iter()
        .map(|(_, ip, port)| format!("{}.{}.{}.{}:{}", ip[0], ip[1], ip[2], ip[3], port))
        .collect::<Vec<_>>()
        .join(",")
}

const PROTO: u8 = 0xE3;
const OP_LOGIN_REQ: u8 = 0x01;
const OP_REJECT: u8 = 0x05;
const OP_SERVER_MESSAGE: u8 = 0x38;
const OP_SERVER_STATUS: u8 = 0x34;
const OP_SERVER_LIST: u8 = 0x37;
const OP_SERVER_IDENT: u8 = 0x41;
const OP_GET_SOURCES: u8 = 0x19;
const OP_FOUND_SOURCES: u8 = 0x42;
const OP_ID_CHANGE: u8 = 0x40;
const CT_NAME: u8 = 0x01;
const CT_VERSION: u8 = 0x11;
const CT_EMULE_VERSION: u8 = 0x0F;
const CT_SERVER_FLAGS: u8 = 0x20;
const SRV_FLAGS: u32 = 0x0001 | 0x0002 | 0x0004 | 0x0008 | 0x0010 | 0x0100;
/// 低于该值的 client id 视为 LowID(不可直连)。
const LOWID_THRESHOLD: u32 = 0x0100_0000;
const OLD_MAX_FILE_SIZE: u64 = 4_290_048_000;
const MAX_ED2K_FILE_SIZE: u64 = 0x40_0000_0000;

pub async fn concurrency(app: &App) -> usize {
    app.db
        .setting(ED2K_CONCURRENCY_KEY)
        .await
        .and_then(|v| v.parse::<usize>().ok())
        .map(|v| v.clamp(1, 16))
        .unwrap_or(DEFAULT_ED2K_CONCURRENCY)
}

/// `ip:port` → `(网络字节序 ip 的 u32 表示, port)`。
fn parse_host_port(s: &str) -> Option<(u32, u16)> {
    let (h, p) = s.rsplit_once(':')?;
    let octets: Vec<u8> = h.split('.').filter_map(|x| x.parse::<u8>().ok()).collect();
    if octets.len() != 4 {
        return None;
    }
    let port = p.parse::<u16>().ok()?;
    Some((
        ((octets[0] as u32) << 24) | ((octets[1] as u32) << 16) | ((octets[2] as u32) << 8) | octets[3] as u32,
        port,
    ))
}

/// eD2K 文件链接解析结果。
pub struct Ed2kLink {
    pub name: String,
    pub size: i64,
    pub hash: [u8; 16],
    /// 链接内联来源(`|s=ip:port|`)。
    pub sources: Vec<(u32, u16)>,
}

pub fn parse_link(url: &str) -> Result<Ed2kLink, String> {
    if !url.to_ascii_lowercase().starts_with("ed2k://") {
        return Err("不是有效的 ed2k 链接".into());
    }
    let parts: Vec<&str> = url.split('|').collect();
    if parts.len() < 5 || parts[1] != "file" {
        return Err("不是有效的 ed2k 文件链接".into());
    }
    let size: i64 = parts[3].trim().parse().map_err(|_| "链接中文件大小无效")?;
    if size <= 0 {
        return Err("链接中文件大小无效".into());
    }
    let hex = parts[4].trim().to_ascii_lowercase();
    if hex.len() != 32 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("链接中 MD4 哈希无效".into());
    }
    let mut hash = [0u8; 16];
    for i in 0..16 {
        hash[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).map_err(|_| "链接中 MD4 哈希无效")?;
    }
    // 文件名:UTF-8 优先,GBK 兜底;分隔符替换为下划线。
    let raw: Vec<u8> = percent_encoding::percent_decode_str(parts[2]).collect();
    let decoded = String::from_utf8(raw.clone())
        .unwrap_or_else(|_| {
            let (cow, _, _) = encoding_rs::GBK.decode(&raw);
            cow.into_owned()
        })
        .replace(['/', '\\', '\0'], "_");
    let name = decoded.trim().to_string();
    let mut sources = Vec::new();
    for seg in parts.iter().skip(5) {
        let seg = seg.trim_start_matches('/');
        if let Some(v) = seg.strip_prefix('s').or_else(|| seg.strip_prefix('S')) {
            if let Some(v) = v.strip_prefix('=') {
                if let Some(sa) = parse_host_port(v) {
                    sources.push(sa);
                }
            }
        }
    }
    Ok(Ed2kLink {
        name: if name.is_empty() { format!("ed2k-{hex}") } else { name },
        size,
        hash,
        sources,
    })
}

// ---------------- server.met 订阅 ----------------

const SERVER_MET_URLS: &[&str] = &[
    "https://upd.emule-security.org/server.met",
    "https://www.shortypower.org/server.met",
];
const SERVER_MET_CACHE_TTL: Duration = Duration::from_secs(24 * 3600);

/// 解析 server.met 二进制格式,返回 `ip:port` 列表(过滤内网 / 保留地址)。
pub fn parse_server_met(bytes: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let Some(&header) = bytes.first() else { return out };
    if header != 0x0E && header != 0xE0 {
        return out;
    }
    if bytes.len() < 5 {
        return out;
    }
    let count = u32::from_le_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]) as usize;
    let mut off = 5usize;
    for _ in 0..count.min(512) {
        let Some(ipb) = bytes.get(off..off + 4) else { break };
        let ip = std::net::Ipv4Addr::new(ipb[0], ipb[1], ipb[2], ipb[3]);
        off += 4;
        let Some(pb) = bytes.get(off..off + 2) else { break };
        let port = u16::from_le_bytes([pb[0], pb[1]]);
        off += 2;
        let Some(tcb) = bytes.get(off..off + 4) else { break };
        let tag_count = u32::from_le_bytes([tcb[0], tcb[1], tcb[2], tcb[3]]) as usize;
        off += 4;
        let mut ok = true;
        for _ in 0..tag_count {
            match skip_met_tag(bytes, off) {
                Some(next) => off = next,
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if !ok {
            break;
        }
        if port != 0 && !ip.is_private() && !ip.is_loopback() && !ip.is_unspecified() {
            out.push(format!("{ip}:{port}"));
        }
    }
    out
}

fn skip_met_tag(bytes: &[u8], off: usize) -> Option<usize> {
    let tag_type = *bytes.get(off)?;
    let name_len = u16::from_le_bytes([*bytes.get(off + 1)?, *bytes.get(off + 2)?]) as usize;
    let mut cur = off + 3 + name_len;
    match tag_type {
        0x02 | 0x06 => {
            let vlen = u16::from_le_bytes([*bytes.get(cur)?, *bytes.get(cur + 1)?]) as usize;
            cur += 2 + vlen;
        }
        0x03 | 0x04 => cur += 4,
        0x05 | 0x09 => cur += 1,
        0x07 => {
            let vlen = u32::from_le_bytes([
                *bytes.get(cur)?,
                *bytes.get(cur + 1)?,
                *bytes.get(cur + 2)?,
                *bytes.get(cur + 3)?,
            ]) as usize;
            cur += 4 + vlen;
        }
        0x08 => cur += 2,
        _ => return None,
    }
    (cur <= bytes.len()).then_some(cur)
}

type Rd = BufReader<OwnedReadHalf>;
type Wr = BufWriter<OwnedWriteHalf>;

async fn send_packet(wr: &mut Wr, proto: u8, op: u8, payload: &[u8]) -> anyhow::Result<()> {
    wr.write_all(&[proto]).await?;
    wr.write_all(&((1 + payload.len()) as u32).to_le_bytes()).await?;
    wr.write_all(&[op]).await?;
    wr.write_all(payload).await?;
    wr.flush().await?;
    Ok(())
}

async fn read_packet(rd: &mut Rd) -> anyhow::Result<(u8, u8, Vec<u8>)> {
    let mut head = [0u8; 5];
    rd.read_exact(&mut head).await?;
    let proto = head[0];
    let len = u32::from_le_bytes(head[1..5].try_into().unwrap()) as usize;
    if !(1..=64 * 1024).contains(&len) {
        anyhow::bail!("异常包长度 {len}");
    }
    let mut body = vec![0u8; len];
    rd.read_exact(&mut body).await?;
    Ok((proto, body[0], body[1..].to_vec()))
}

fn push_tag(out: &mut Vec<u8>, ty: u8, name: &[u8], value: &[u8]) {
    out.push(ty);
    out.extend_from_slice(&(name.len() as u16).to_le_bytes());
    out.extend_from_slice(name);
    out.extend_from_slice(value);
}

/// eD2K 用户哈希:固定文件 `data/ed2k-user-hash.bin`,首次生成后复用
/// (与历史版本相同的落盘位置,保持被服务器识别的连续性)。
fn user_hash() -> [u8; 16] {
    use std::sync::OnceLock;
    static H: OnceLock<[u8; 16]> = OnceLock::new();
    *H.get_or_init(|| {
        let path = PathBuf::from("data/ed2k-user-hash.bin");
        if let Ok(b) = std::fs::read(&path) {
            if b.len() == 16 && b.iter().any(|&x| x != 0) {
                let mut a = [0u8; 16];
                a.copy_from_slice(&b);
                return a;
            }
        }
        use rand::RngExt;
        let mut a = rand::rng().random::<[u8; 16]>();
        // eMule 客户端哈希的固定字节,便于被识别为合法客户端。
        a[5] = (a[5] & 0xF0) | 0x0E;
        let _ = std::fs::create_dir_all(path.parent().unwrap_or_else(|| Path::new(".")));
        let _ = std::fs::write(&path, a);
        a
    })
}

fn login_payload(client_port: u16) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&user_hash());
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(&client_port.to_le_bytes());
    v.extend_from_slice(&4u32.to_le_bytes());
    push_tag(&mut v, 0x03, &[CT_VERSION], &60u32.to_le_bytes());
    push_tag(&mut v, 0x03, &[CT_SERVER_FLAGS], &SRV_FLAGS.to_le_bytes());
    push_tag(&mut v, 0x02, &[CT_NAME], b"NDisk");
    push_tag(&mut v, 0x03, &[CT_EMULE_VERSION], &0x3C000013u32.to_le_bytes());
    v
}

/// 服务器列表:24h 缓存(server.met)优先,自定义设置置顶,默认列表兜底。
pub async fn ed2k_servers(app: &App) -> Vec<String> {
    let cache = cached::cache_path(&app.data_dir, "ed2k-servers.txt");
    let mut out: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut pushed = false;
    if let Some(text) = cached::read_fresh_text(&cache, SERVER_MET_CACHE_TTL) {
        for l in text.lines() {
            let l = l.trim();
            if !l.is_empty() && seen.insert(l.to_string()) {
                out.push(l.to_string());
            }
        }
    } else {
        for url in SERVER_MET_URLS {
            if let Ok(Ok(resp)) = tokio::time::timeout(Duration::from_secs(20), reqwest::get(*url)).await {
                if let Ok(bytes) = resp.bytes().await {
                    for l in parse_server_met(&bytes) {
                        if seen.insert(l.clone()) {
                            out.push(l);
                            pushed = true;
                        }
                    }
                }
            }
        }
        if pushed {
            cached::write_cache(&cache, out.join("\n").as_bytes());
        }
    }
    let custom = app.db.setting(ED2K_SERVERS_KEY).await.unwrap_or_default();
    for l in custom.split([',', ';', '\n']).map(str::trim) {
        if !l.is_empty() && !l.to_lowercase().starts_with("public-ip-") && seen.insert(l.to_string()) {
            if let Some(pos) = out.iter().position(|x| x == l) {
                out.remove(pos);
            }
            out.insert(0, l.to_string());
        }
    }
    if out.is_empty() {
        for l in default_servers().split(',') {
            out.push(l.to_string());
        }
    }
    out
}

/// 连接一台 eD2K 服务器:登录 → GetSources → 收集 FoundSources。
async fn sources_from_server(addr: &str, hash: &[u8; 16], size: i64) -> anyhow::Result<Vec<(u32, u16)>> {
    let size = size.max(0) as u64;
    if size > MAX_ED2K_FILE_SIZE {
        anyhow::bail!("超过 eD2K 协议上限(256GB)的文件");
    }
    let stream = tokio::time::timeout(Duration::from_secs(10), TcpStream::connect(addr)).await??;
    let (rd, wr) = stream.into_split();
    let mut rd = BufReader::new(rd);
    let mut wr = BufWriter::new(wr);
    send_packet(&mut wr, PROTO, OP_LOGIN_REQ, &login_payload(4662)).await?;

    for _ in 0..32 {
        let (_proto, op, _payload) = match tokio::time::timeout(Duration::from_secs(10), read_packet(&mut rd)).await {
            Ok(Ok(p)) => p,
            Ok(Err(e)) => return Err(e),
            Err(_) => anyhow::bail!("登录应答超时"),
        };
        if op == OP_ID_CHANGE {
            break;
        }
        if op == OP_REJECT {
            anyhow::bail!("服务器拒绝登录");
        }
    }

    // >4GB 的文件用 64 位大小扩展(0x35 协议变体)。
    let mut req = Vec::new();
    req.extend_from_slice(hash);
    if size > OLD_MAX_FILE_SIZE {
        req.extend_from_slice(&0u32.to_le_bytes());
        req.extend_from_slice(&size.to_le_bytes());
    } else {
        req.extend_from_slice(&(size as u32).to_le_bytes());
    }
    send_packet(&mut wr, PROTO, OP_GET_SOURCES, &req).await?;

    let deadline = now_secs() + 30;
    let mut sources = Vec::new();
    let mut frames = 0usize;
    while now_secs() < deadline && frames < 32 {
        frames += 1;
        let (_proto, op, payload) = match tokio::time::timeout(Duration::from_secs(15), read_packet(&mut rd)).await {
            Ok(Ok(p)) => p,
            _ => break,
        };
        match op {
            OP_REJECT => anyhow::bail!("服务器拒绝查源请求"),
            OP_FOUND_SOURCES => {
                if payload.len() < 20 || payload[..16] != *hash {
                    continue;
                }
                let count = u32::from_le_bytes([payload[16], payload[17], payload[18], payload[19]]) as usize;
                // 6 字节条目(ip+port)或 8 字节条目(带 crypt flag)。
                let six = 20 + count * 6 <= payload.len() && payload.len() < 20 + count * 8;
                for i in 0..count {
                    let off = 20 + i * if six { 6 } else { 8 };
                    if off + 6 > payload.len() {
                        break;
                    }
                    let id = u32::from_le_bytes([payload[off], payload[off + 1], payload[off + 2], payload[off + 3]]);
                    let port = u16::from_le_bytes([payload[off + 4], payload[off + 5]]);
                    let mut ok = id >= LOWID_THRESHOLD && port > 0;
                    if !six && off + 7 < payload.len() {
                        ok &= payload[off + 6] & 0x01 != 0;
                    }
                    if ok {
                        let b = id.to_le_bytes();
                        let repr = ((b[0] as u32) << 24) | ((b[1] as u32) << 16) | ((b[2] as u32) << 8) | (b[3] as u32);
                        if !sources.contains(&(repr, port)) {
                            sources.push((repr, port));
                        }
                    }
                }
                if !sources.is_empty() {
                    break;
                }
            }
            OP_SERVER_MESSAGE | OP_SERVER_LIST | OP_SERVER_IDENT | OP_SERVER_STATUS => {}
            _ => {}
        }
    }
    Ok(sources)
}

/// 查源总入口:并行发起"服务器查源"与"KAD 查源",谁先给出非空结果用谁。
pub(crate) async fn find_sources(app: &App, hash: &[u8; 16], size: i64) -> anyhow::Result<Vec<(u32, u16)>> {
    let kad_on = super::kad::kad_enabled(app).await;
    type ChannelResult = (&'static str, anyhow::Result<Vec<(u32, u16)>>);
    let mut join: tokio::task::JoinSet<ChannelResult> = tokio::task::JoinSet::new();
    {
        let app2 = app.clone();
        let hash = *hash;
        join.spawn(async move { ("服务器", query_servers(&app2, &hash, size).await) });
    }
    if kad_on {
        let app2 = app.clone();
        let hash = *hash;
        join.spawn(async move {
            let r = match tokio::time::timeout(Duration::from_secs(45), super::kad::find_sources(&app2, &hash, size)).await {
                Ok(r) => r,
                Err(_) => Err("45s 内未收敛".to_string()),
            };
            (
                "KAD",
                r.map(|list| {
                    list.iter()
                        .map(|s| {
                            (
                                ((s.ip[0] as u32) << 24) | ((s.ip[1] as u32) << 16) | ((s.ip[2] as u32) << 8) | (s.ip[3] as u32),
                                s.tcp,
                            )
                        })
                        .collect::<Vec<(u32, u16)>>()
                })
                .map_err(|e| anyhow::anyhow!(e)),
            )
        });
    }
    let mut errs: Vec<String> = Vec::new();
    while let Some(res) = join.join_next().await {
        let Ok((who, r)) = res else { continue };
        match r {
            Ok(v) if !v.is_empty() => {
                join.abort_all();
                return Ok(v);
            }
            Ok(_) => errs.push(format!("{who} 无来源")),
            Err(e) => errs.push(format!("{who}: {e}")),
        }
    }
    anyhow::bail!(
        "未找到下载来源({});可在后台管理 → 下载设置中更新 eD2K 服务器列表",
        errs.join("; ")
    )
}

/// 并发查询全部服务器(信号量限流),凑满 10 个来源或宽限 5 秒后返回。
async fn query_servers(app: &App, hash: &[u8; 16], size: i64) -> anyhow::Result<Vec<(u32, u16)>> {
    let list = ed2k_servers(app).await;
    if list.is_empty() {
        anyhow::bail!("未配置 eD2K 服务器,请在后台管理 → 下载设置中填写");
    }
    let deadline = now_secs() + 45;
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(12));
    let mut join: tokio::task::JoinSet<Result<Vec<(u32, u16)>, String>> = tokio::task::JoinSet::new();
    for addr in &list {
        let sem = sem.clone();
        let addr = addr.clone();
        let hash = *hash;
        join.spawn(async move {
            let _permit = sem.acquire().await;
            match tokio::time::timeout(Duration::from_secs(40), sources_from_server(&addr, &hash, size)).await {
                Ok(Ok(v)) if !v.is_empty() => Ok(v),
                Ok(Ok(_)) => Err(format!("{addr} 无来源")),
                Ok(Err(e)) => {
                    let msg = e.to_string();
                    let msg = if msg.contains("deadline") || msg.contains("timed out") {
                        "连接超时".to_string()
                    } else {
                        msg
                    };
                    Err(format!("{addr}: {msg}"))
                }
                Err(_) => Err(format!("{addr} 响应超时")),
            }
        });
    }
    let mut agg: Vec<(u32, u16)> = Vec::new();
    let mut errs: Vec<String> = Vec::new();
    let mut grace_deadline = 0i64;
    loop {
        let bound = if grace_deadline > 0 { grace_deadline } else { deadline };
        let wait = Duration::from_secs((bound - now_secs()).max(1) as u64);
        match tokio::time::timeout(wait, join.join_next()).await {
            Ok(Some(Ok(r))) => match r {
                Ok(v) => {
                    for s in v {
                        if !agg.contains(&s) {
                            agg.push(s);
                        }
                    }
                    if agg.len() >= 10 {
                        break;
                    }
                    if grace_deadline == 0 {
                        grace_deadline = now_secs() + 5;
                    }
                }
                Err(e) => errs.push(e),
            },
            Ok(Some(Err(_))) => {}
            Ok(None) => break,
            Err(_) => {
                if grace_deadline > 0 && !agg.is_empty() {
                    break;
                }
                if join.is_empty() {
                    break;
                }
                continue;
            }
        }
        if grace_deadline > 0 && now_secs() >= grace_deadline {
            break;
        }
        if now_secs() >= deadline && !agg.is_empty() {
            break;
        }
    }
    if !agg.is_empty() {
        return Ok(agg);
    }
    anyhow::bail!("服务器查源失败({})", errs.join("; "))
}

// ---------------- 块级下载调度 ----------------

#[derive(Clone, Default)]
pub(crate) struct Ed2kPart {
    pub(crate) done: bool,
    pub(crate) inflight: bool,
    pub(crate) md4: Option<[u8; 16]>,
}

pub(crate) struct Ed2kShared {
    pub(crate) file: tokio::sync::Mutex<tokio::fs::File>,
    pub(crate) parts: tokio::sync::Mutex<Vec<Ed2kPart>>,
    pub(crate) hashset: tokio::sync::OnceCell<Vec<[u8; 16]>>,
    pub(crate) received: std::sync::atomic::AtomicU64,
    pub(crate) fails: tokio::sync::Mutex<std::collections::HashMap<(u32, u16), u32>>,
}

/// 单个来源 worker:领取块 → 对端下载 → 校验块 MD4 → 写回共享进度。
/// 块校验失败 / 对端无文件计 2 次失败(立即拉黑);网络错误计 1 次。
pub(crate) async fn peer_worker(
    addr: (u32, u16),
    hash: [u8; 16],
    size: i64,
    parts_total: usize,
    shared: std::sync::Arc<Ed2kShared>,
) -> Result<(), String> {
    let octets = [
        ((addr.0 >> 24) & 0xFF) as u8,
        ((addr.0 >> 16) & 0xFF) as u8,
        ((addr.0 >> 8) & 0xFF) as u8,
        (addr.0 & 0xFF) as u8,
    ];
    let saddr = std::net::SocketAddr::from((std::net::Ipv4Addr::new(octets[0], octets[1], octets[2], octets[3]), addr.1));
    let mut sess = match super::peer::PeerSession::open(saddr, &hash, size).await {
        Ok(s) => s,
        Err(e) => {
            let msg = match e {
                super::peer::PeerErr::NoFile => "对端没有此文件".to_string(),
                super::peer::PeerErr::Integrity(m) => format!("完整性违规: {m}"),
                super::peer::PeerErr::Net(m) => m,
            };
            return Err(msg);
        }
    };
    if (size.max(0) as u64) >= super::peer::PART_SIZE {
        match shared.hashset.get_or_try_init(|| sess.fetch_hashset()).await {
            Ok(h) if h.len() == parts_total => {}
            Ok(_) => return Err("hashset 块数与文件不符".into()),
            Err(e) => return Err(format!("hashset 获取失败: {e}")),
        }
    }
    loop {
        let part = {
            let ps = shared.parts.lock().await;
            (0..parts_total).find(|&i| !ps[i].done && !ps[i].inflight && sess.has_part(i, parts_total))
        };
        let Some(part) = part else { break };
        shared.parts.lock().await[part].inflight = true;
        match sess.download_part(part, &shared.file, &shared.received).await {
            Ok(md4) => {
                if let Some(expect) = shared.hashset.get().and_then(|h| h.get(part)) {
                    if md4 != *expect {
                        shared.parts.lock().await[part].inflight = false;
                        // 块校验失败说明来源数据损坏,直接计满拉黑。
                        *shared.fails.lock().await.entry(addr).or_insert(0) += 2;
                        return Err("块 MD4 校验失败(来源数据损坏)".into());
                    }
                }
                let mut ps = shared.parts.lock().await;
                ps[part].done = true;
                ps[part].inflight = false;
                ps[part].md4 = Some(md4);
                let done = ps.iter().filter(|p| p.done).count();
                drop(ps);
                tracing::info!("[离线下载] eD2K 块 {}/{} 完成({})", done, parts_total, saddr);
            }
            Err(super::peer::PeerErr::NoFile) => {
                shared.parts.lock().await[part].inflight = false;
                let mut fails = shared.fails.lock().await;
                *fails.entry(addr).or_insert(0) += 2;
                return Err("对端没有此文件".into());
            }
            Err(super::peer::PeerErr::Integrity(m)) => {
                shared.parts.lock().await[part].inflight = false;
                let mut fails = shared.fails.lock().await;
                *fails.entry(addr).or_insert(0) += 2;
                return Err(format!("完整性违规: {m}"));
            }
            Err(super::peer::PeerErr::Net(m)) => {
                shared.parts.lock().await[part].inflight = false;
                return Err(m);
            }
        }
    }
    Ok(())
}

// ---------------- MD4(仅用于 eD2K 文件 / 块校验) ----------------

pub fn md4(data: &[u8]) -> [u8; 16] {
    let mut msg = data.to_vec();
    let bitlen = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bitlen.to_le_bytes());

    const S1: [u32; 4] = [3, 7, 11, 19];
    const S2: [u32; 4] = [3, 5, 9, 13];
    const S3: [u32; 4] = [3, 9, 11, 15];
    const X2: [usize; 16] = [0, 4, 8, 12, 1, 5, 9, 13, 2, 6, 10, 14, 3, 7, 11, 15];
    const X3: [usize; 16] = [0, 8, 4, 12, 2, 10, 6, 14, 1, 9, 5, 13, 3, 11, 7, 15];

    let (mut a0, mut b0, mut c0, mut d0) = (0x67452301u32, 0xefcdab89u32, 0x98badcfeu32, 0x10325476u32);
    for chunk in msg.as_chunks::<64>().0 {
        let mut m = [0u32; 16];
        for (i, w) in m.iter_mut().enumerate() {
            *w = u32::from_le_bytes([chunk[i * 4], chunk[i * 4 + 1], chunk[i * 4 + 2], chunk[i * 4 + 3]]);
        }
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for j in 0..16 {
            let f = (b & c) | (!b & d);
            let t = a.wrapping_add(f).wrapping_add(m[j]);
            a = t.rotate_left(S1[j % 4]);
            (a, b, c, d) = (d, a, b, c);
        }
        for j in 0..16 {
            let g = (b & c) | (b & d) | (c & d);
            let t = a.wrapping_add(g).wrapping_add(m[X2[j]]).wrapping_add(0x5A827999);
            a = t.rotate_left(S2[j % 4]);
            (a, b, c, d) = (d, a, b, c);
        }
        for j in 0..16 {
            let h = b ^ c ^ d;
            let t = a.wrapping_add(h).wrapping_add(m[X3[j]]).wrapping_add(0x6ED9EBA1);
            a = t.rotate_left(S3[j % 4]);
            (a, b, c, d) = (d, a, b, c);
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&a0.to_le_bytes());
    out[4..8].copy_from_slice(&b0.to_le_bytes());
    out[8..12].copy_from_slice(&c0.to_le_bytes());
    out[12..16].copy_from_slice(&d0.to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(h: [u8; 16]) -> String {
        h.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn md4_vectors() {
        assert_eq!(hex(md4(b"")), "31d6cfe0d16ae931b73c59d7e0c089c0");
        assert_eq!(hex(md4(b"a")), "bde52cb31de33e46245e05fbdbd6fb24");
        assert_eq!(hex(md4(b"abc")), "a448017aaf21d8525fc10ae87aa6729d");
        assert_eq!(hex(md4(b"message digest")), "d9130a8164549fe818874806e1c7014b");
    }

    #[test]
    fn parse_ed2k_link() {
        let l = parse_link("ed2k://|file|Ubuntu.iso|1024|A448017AAF21D8525FC10AE87AA6729D|h=X|/").unwrap();
        assert_eq!(l.name, "Ubuntu.iso");
        assert_eq!(l.size, 1024);
        assert_eq!(hex(l.hash), "a448017aaf21d8525fc10ae87aa6729d");
        assert!(parse_link("ed2k://|server|176.103.48.36|4184|/").is_err());
    }

    #[test]
    fn parse_link_inline_sources() {
        let l = parse_link("ed2k://|file|a.bin|10|A448017AAF21D8525FC10AE87AA6729D|s=1.2.3.4:1234|/").unwrap();
        assert_eq!(l.sources, vec![(0x01020304, 1234)]);
    }

    #[test]
    fn server_met_minimal() {
        // 1 台服务器,0 个标签:header(0x0E) + count(1) + ip + port + tagcount(0)
        let mut b = vec![0x0Eu8];
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&[176, 103, 48, 36]);
        b.extend_from_slice(&4184u16.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(parse_server_met(&b), vec!["176.103.48.36:4184"]);
    }
}
