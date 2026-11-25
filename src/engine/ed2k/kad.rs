//! KAD(Kademlia over eD2K UDP):引导(nodes.dat 订阅)→ FindNode 收敛 →
//! 向最近节点发起源搜索 → 解析 SearchResults 里的来源标签。
//! 节点缓存落盘(`kad-nodes.dat` / `kad-contacts.json`)。

use std::time::{Duration, Instant};

use flate2::read::ZlibDecoder;
use serde_json::json;
use std::io::Read;

use crate::app::App;
use crate::domain::cached;

pub const KAD_ENABLED_KEY: &str = "offline_kad_enabled";
pub const KAD_NODES_URL_KEY: &str = "offline_kad_nodes_url";
pub const KAD_NODES_URLS_KEY: &str = "offline_kad_nodes_urls";
pub const DEFAULT_NODES_URL: &str = "https://upd.emule-security.org/nodes.dat";

const PROTO_KAD: u8 = 0xE4;
const PROTO_KAD_PACKED: u8 = 0xE5;
const OP_BOOTSTRAP_REQ: u8 = 0x01;
const OP_HELLO_REQ: u8 = 0x11;
const KADEMLIA_HELLO_VER: u8 = 5;
const KADEMLIA_BOOT_VER: u8 = 6;
const OP_BOOTSTRAP_RES: u8 = 0x09;
const OP_HELLO_RES: u8 = 0x19;
const OP_KAD_REQ: u8 = 0x21;
const OP_KAD_RES: u8 = 0x29;
const OP_SEARCH_SOURCE_REQ: u8 = 0x34;
const OP_SEARCH_RES: u8 = 0x3B;

const TAG_SOURCETYPE: u8 = 0xFF;
const TAG_SOURCEIP: u8 = 0xFE;
const TAG_SOURCEPORT: u8 = 0xFD;

/// 一个查到的下载来源。
#[derive(Clone, Debug)]
pub struct KadSource {
    pub ip: [u8; 4],
    pub tcp: u16,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct KadNode {
    /// 线上字节序的 KadID。
    id_wire: [u8; 16],
    ip: [u8; 4],
    udp: u16,
    ver: u8,
}

/// KAD 的 KadID 与网络字节序存在 4 字节内反转,统一转成内部表示再算 XOR 距离。
fn raw_from_wire(wire: &[u8; 16]) -> [u8; 16] {
    let mut r = [0u8; 16];
    for i in 0..16 {
        r[(i / 4) * 4 + 3 - (i % 4)] = wire[i];
    }
    r
}

fn xor_distance(a_raw: &[u8; 16], b_raw: &[u8; 16]) -> [u8; 16] {
    let mut d = [0u8; 16];
    for i in 0..16 {
        d[i] = a_raw[i] ^ b_raw[i];
    }
    d
}

fn nodes_cache_path(app: &App) -> std::path::PathBuf {
    cached::cache_path(&app.data_dir, "kad-nodes.dat")
}

async fn load_bootstrap_nodes(app: &App) -> Vec<KadNode> {
    let cache = nodes_cache_path(app);
    if let Some(data) = cached::read_fresh(&cache, Duration::from_secs(24 * 3600)) {
        let nodes = parse_nodes_dat(&data);
        if nodes.len() >= 8 {
            return nodes;
        }
    }
    let mut urls_raw = app.db.setting(KAD_NODES_URLS_KEY).await.unwrap_or_default();
    if urls_raw.trim().is_empty() {
        urls_raw = app
            .db
            .setting(KAD_NODES_URL_KEY)
            .await
            .unwrap_or_else(|| DEFAULT_NODES_URL.to_string());
    }
    let urls = subscription_urls(&urls_raw);
    if urls.is_empty() {
        return Vec::new();
    }
    // 各订阅地址并行拉取(单个失败返回空数据,不影响其余);仍按地址顺序合并去重。
    let datas = futures_util::future::join_all(urls.into_iter().take(5).map(|url| async move {
        match tokio::time::timeout(Duration::from_secs(20), reqwest::get(url.as_str())).await {
            Ok(Ok(resp)) => resp.bytes().await.map(|b| b.to_vec()).unwrap_or_default(),
            _ => Vec::new(),
        }
    }))
    .await;
    let mut seen = std::collections::HashSet::new();
    let mut merged: Vec<KadNode> = Vec::new();
    for data in datas {
        for n in parse_nodes_dat(&data) {
            if seen.insert((n.ip, n.udp)) {
                merged.push(n);
            }
        }
    }
    if !merged.is_empty() {
        // 保存为最简 nodes.dat 版本 1 结构,供后续直接复用。
        let mut out: Vec<u8> = Vec::new();
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&(merged.len() as u32).to_le_bytes());
        for n in &merged {
            out.extend_from_slice(&n.id_wire);
            out.push(n.ip[3]);
            out.push(n.ip[2]);
            out.push(n.ip[1]);
            out.push(n.ip[0]);
            out.extend_from_slice(&n.udp.to_le_bytes());
            out.extend_from_slice(&n.udp.to_le_bytes());
            out.push(8u8); // 空标签计数
        }
        cached::write_cache(&cache, &out);
        merged.truncate(256);
        return merged;
    }
    std::fs::read(&cache).map(|d| parse_nodes_dat(&d)).unwrap_or_default()
}

/// 订阅地址支持裸 http(s) URL 与 `ed2k://|nodeslist|URL|/` 包装形式。
fn subscription_urls(raw: &str) -> Vec<String> {
    raw.split([',', ';', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|u| {
            if u.to_ascii_lowercase().starts_with("ed2k://|nodeslist|") {
                let inner = u.split('|').nth(4).unwrap_or("");
                inner.trim_end_matches('/').to_string()
            } else {
                u.to_string()
            }
        })
        .filter(|u| u.starts_with("http://") || u.starts_with("https://"))
        .collect()
}

/// 解析 nodes.dat(版本 1/2/3 与传统 0 头两种布局)。
fn parse_nodes_dat(data: &[u8]) -> Vec<KadNode> {
    let mut nodes = Vec::new();
    let rd = |off: usize| -> u32 { u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]]) };
    if data.len() < 8 {
        return nodes;
    }
    let mut num = rd(0);
    let mut off = 4usize;
    let mut extra = 0usize;
    if num == 0 {
        let ver = rd(off);
        off += 4;
        if ver != 1 && ver != 2 && ver != 3 {
            return nodes;
        }
        extra = if ver >= 2 { 9 } else { 0 };
        num = rd(off);
        off += 4;
        if ver == 3 {
            let edition = rd(off);
            off += 4;
            if edition == 1 {
                extra = 0;
            }
        }
    }
    let stride = 25 + extra;
    for _ in 0..num {
        if off + stride > data.len() {
            break;
        }
        let mut id_wire = [0u8; 16];
        id_wire.copy_from_slice(&data[off..off + 16]);
        let ipb = &data[off + 16..off + 20];
        let ip = [ipb[3], ipb[2], ipb[1], ipb[0]];
        let udp = u16::from_le_bytes([data[off + 20], data[off + 21]]);
        let ver = data[off + 24];
        if ip[0] != 0 && ip != [127, 0, 0, 1] && udp > 0 && ver >= 2 {
            nodes.push(KadNode { id_wire, ip, udp, ver });
        }
        off += stride;
    }
    nodes.sort_by_key(|n| n.id_wire);
    nodes.dedup_by(|a, b| a.id_wire == b.id_wire);
    nodes.truncate(64);
    nodes
}

struct KadSession {
    socket: Arc<tokio::net::UdpSocket>,
    contacts: Vec<KadNode>,
    queried: std::collections::HashSet<[u8; 16]>,
    sources: Vec<KadSource>,
    /// 目标文件哈希(内部字节序)。
    target_raw: [u8; 16],
}

use std::sync::Arc;

impl KadSession {
    fn send(&self, opcode: u8, payload: &[u8], node: &KadNode) {
        let mut pkt = Vec::with_capacity(payload.len() + 2);
        pkt.push(PROTO_KAD);
        pkt.push(opcode);
        pkt.extend_from_slice(payload);
        let addr = std::net::SocketAddr::from((
            std::net::Ipv4Addr::new(node.ip[0], node.ip[1], node.ip[2], node.ip[3]),
            node.udp,
        ));
        let _ = self.socket.try_send_to(&pkt, addr);
    }

    fn add_contact(&mut self, id_wire: [u8; 16], ip: [u8; 4], udp: u16, ver: u8) {
        if ip[0] == 0 || udp == 0 || ver < 2 {
            return;
        }
        if self.contacts.iter().any(|c| c.id_wire == id_wire) {
            return;
        }
        self.contacts.push(KadNode { id_wire, ip, udp, ver });
    }

    /// 按 XOR 距离取最近的 n 个满足条件的节点。
    fn closest(&self, n: usize, pred: impl Fn(&KadNode) -> bool) -> Vec<KadNode> {
        let mut scored: Vec<(&KadNode, [u8; 16])> = self
            .contacts
            .iter()
            .filter(|c| pred(c))
            .map(|c| (c, xor_distance(&self.target_raw, &raw_from_wire(&c.id_wire))))
            .collect();
        scored.sort_by_key(|(_, d)| *d);
        scored.into_iter().take(n).map(|(c, _)| c.clone()).collect()
    }

    fn add_source(&mut self, ip: [u8; 4], tcp: u16) {
        if tcp == 0 || ip[0] == 0 {
            return;
        }
        if self.sources.iter().any(|s| s.ip == ip && s.tcp == tcp) {
            return;
        }
        self.sources.push(KadSource { ip, tcp });
    }

    fn on_packet(&mut self, from: std::net::SocketAddr, data: &[u8]) {
        if data.len() < 3 {
            return;
        }
        let mut proto = data[0];
        let opcode = data[1];
        let mut payload = data[2..].to_vec();
        if proto == PROTO_KAD_PACKED {
            let mut dec = ZlibDecoder::new(&payload[..]);
            let mut out = Vec::new();
            if dec.read_to_end(&mut out).is_err() {
                return;
            }
            payload = out;
            proto = PROTO_KAD;
        }
        if proto != PROTO_KAD {
            return;
        }
        let src_ip = match from.ip() {
            std::net::IpAddr::V4(v) => v.octets(),
            _ => return,
        };
        match opcode {
            OP_BOOTSTRAP_RES => {
                if payload.len() < 21 {
                    return;
                }
                let n = u16::from_le_bytes([payload[19], payload[20]]) as usize;
                self.add_contact(payload[0..16].try_into().unwrap(), src_ip, from.port(), payload[18]);
                let mut off = 21usize;
                for _ in 0..n {
                    if off + 25 > payload.len() {
                        break;
                    }
                    let id_wire: [u8; 16] = payload[off..off + 16].try_into().unwrap();
                    let ip = [payload[off + 19], payload[off + 18], payload[off + 17], payload[off + 16]];
                    let udp = u16::from_le_bytes([payload[off + 20], payload[off + 21]]);
                    let ver = payload[off + 24];
                    self.add_contact(id_wire, ip, udp, ver);
                    off += 25;
                }
            }
            OP_HELLO_RES => {
                if payload.len() < 21 {
                    return;
                }
                self.add_contact(
                    payload[0..16].try_into().unwrap(),
                    src_ip,
                    u16::from_le_bytes([payload[16], payload[17]]),
                    payload[18],
                );
            }
            OP_KAD_RES => {
                if payload.len() < 17 {
                    return;
                }
                let n = payload[16] as usize;
                let mut off = 17usize;
                for _ in 0..n {
                    if off + 25 > payload.len() {
                        break;
                    }
                    let id_wire: [u8; 16] = payload[off..off + 16].try_into().unwrap();
                    let ip = [payload[off + 19], payload[off + 18], payload[off + 17], payload[off + 16]];
                    let udp = u16::from_le_bytes([payload[off + 20], payload[off + 21]]);
                    let ver = payload[off + 24];
                    self.add_contact(id_wire, ip, udp, ver);
                    off += 25;
                }
            }
            OP_SEARCH_RES => {
                if payload.len() < 36 {
                    return;
                }
                let n = u16::from_le_bytes([payload[32], payload[33]]) as usize;
                let mut off = 34usize;
                for _ in 0..n {
                    if off + 16 > payload.len() {
                        break;
                    }
                    off += 16;
                    let tags = match read_tag_list(&payload, &mut off) {
                        Some(t) => t,
                        None => break,
                    };
                    let mut stype = 0u8;
                    let mut sip: [u8; 4] = [0, 0, 0, 0];
                    let mut stcp = 0u16;
                    for (id, val) in &tags {
                        match *id {
                            TAG_SOURCETYPE => stype = val.first().copied().unwrap_or(0),
                            TAG_SOURCEIP => {
                                if val.len() < 4 {
                                    continue;
                                }
                                let v = u32::from_le_bytes([val[0], val[1], val[2], val[3]]);
                                sip = [
                                    ((v >> 24) & 0xFF) as u8,
                                    ((v >> 16) & 0xFF) as u8,
                                    ((v >> 8) & 0xFF) as u8,
                                    (v & 0xFF) as u8,
                                ];
                            }
                            TAG_SOURCEPORT if val.len() >= 2 => {
                                stcp = u16::from_le_bytes([val[0], val[1]]);
                            }
                            _ => {}
                        }
                    }
                    // 类型 0/1/4 都是可直连的文件来源。
                    if stype == 0 || stype == 1 || stype == 4 {
                        tracing::info!("[KAD] 来源: {}.{}.{}.{}:{}", sip[0], sip[1], sip[2], sip[3], stcp);
                        self.add_source(sip, stcp);
                    }
                }
            }
            _ => {}
        }
    }
}

fn read_tag_list(data: &[u8], off: &mut usize) -> Option<Vec<(u8, Vec<u8>)>> {
    let mut tags = Vec::new();
    let count = *data.get(*off)?;
    *off += 1;
    for _ in 0..count {
        if *off + 3 > data.len() {
            return None;
        }
        let ty = data[*off];
        let nlen = u16::from_le_bytes([data[*off + 1], data[*off + 2]]) as usize;
        *off += 3;
        if nlen == 1 {
            let id = *data.get(*off)?;
            *off += 1;
            let val = read_tag_value(data, off, ty)?;
            tags.push((id, val));
        } else {
            *off += nlen;
            let val = read_tag_value(data, off, ty)?;
            tags.push((0, val));
        }
    }
    Some(tags)
}

/// 按 eD2K 标签类型完整推进偏移并取值(HASH16/STRING/UINT32/UINT16/UINT8/UINT64/BSOB)。
fn read_tag_value(data: &[u8], off: &mut usize, ty: u8) -> Option<Vec<u8>> {
    const TAGTYPE_HASH16: u8 = 0x01;
    const TAGTYPE_STRING: u8 = 0x02;
    const TAGTYPE_UINT32: u8 = 0x03;
    const TAGTYPE_UINT16: u8 = 0x08;
    const TAGTYPE_UINT8: u8 = 0x09;
    const TAGTYPE_BSOB: u8 = 0x0A;
    const TAGTYPE_UINT64: u8 = 0x0B;
    let bytes = match ty {
        TAGTYPE_UINT32 => {
            if *off + 4 > data.len() {
                return None;
            }
            let b = data[*off..*off + 4].to_vec();
            *off += 4;
            b
        }
        TAGTYPE_UINT16 => {
            if *off + 2 > data.len() {
                return None;
            }
            let b = data[*off..*off + 2].to_vec();
            *off += 2;
            b
        }
        TAGTYPE_UINT8 => {
            if *off + 1 > data.len() {
                return None;
            }
            let b = data[*off..*off + 1].to_vec();
            *off += 1;
            b
        }
        TAGTYPE_UINT64 => {
            if *off + 8 > data.len() {
                return None;
            }
            let b = data[*off..*off + 8].to_vec();
            *off += 8;
            b
        }
        TAGTYPE_STRING | TAGTYPE_BSOB => {
            if *off + 2 > data.len() {
                return None;
            }
            let l = u16::from_le_bytes([data[*off], data[*off + 1]]) as usize;
            *off += 2;
            if *off + l > data.len() {
                return None;
            }
            let b = data[*off..*off + l].to_vec();
            *off += l;
            b
        }
        TAGTYPE_HASH16 => {
            if *off + 16 > data.len() {
                return None;
            }
            let b = data[*off..*off + 16].to_vec();
            *off += 16;
            b
        }
        _ => return None,
    };
    Some(bytes)
}

/// 查源主流程:引导 → FindNode 收敛(阶段 0/1)→ 向最近 12 节点发源搜索(阶段 2)。
/// 全程 100 秒上限;凑够 8 个来源或连续两轮无新增即提前返回。
pub async fn find_sources(app: &App, hash: &[u8; 16], size: i64) -> Result<Vec<KadSource>, String> {
    use tokio::sync::OnceCell as AsyncOnceCell;
    static SOCK: AsyncOnceCell<Arc<tokio::net::UdpSocket>> = AsyncOnceCell::const_new();
    // 全局共用一个 UDP socket(便于 NAT 映射稳定);搜索本身串行化。
    static SEARCH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _guard = SEARCH_LOCK.lock().await;
    let socket: Arc<tokio::net::UdpSocket> = SOCK
        .get_or_try_init(|| async {
            tokio::net::UdpSocket::bind("0.0.0.0:0")
                .await
                .map(Arc::new)
                .map_err(|e| format!("KAD UDP 绑定失败: {e}"))
        })
        .await?
        .clone();
    let mut session_raw = [0u8; 16];
    session_raw.copy_from_slice(hash);
    let bootstrap = load_bootstrap_nodes(app).await;
    let cache_file = cached::cache_path(&app.data_dir, "kad-contacts.json");
    let cached_contacts: Vec<KadNode> = std::fs::read(&cache_file)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let warm = cached_contacts.len() >= 12;
    let mut session = KadSession {
        socket,
        contacts: Vec::new(),
        queried: std::collections::HashSet::new(),
        sources: Vec::new(),
        target_raw: session_raw,
    };

    session.contacts = cached_contacts.clone();
    let known: std::collections::HashSet<[u8; 16]> = cached_contacts.iter().map(|n| n.id_wire).collect();
    let mut pending: Vec<KadNode> = if warm {
        bootstrap.iter().filter(|n| !known.contains(&n.id_wire)).take(6).cloned().collect()
    } else {
        if bootstrap.is_empty() {
            return Err("无法获取 KAD 引导节点(nodes.dat 拉取失败)".into());
        }
        bootstrap.clone()
    };
    let started = Instant::now();
    let total_deadline = started + Duration::from_secs(100);
    let mut buf = vec![0u8; 65536];
    let mut sent_bootstrap: std::collections::HashSet<[u8; 16]> = Default::default();
    let mut best_dist: Option<[u8; 16]> = None;
    let mut stalls = 0u32;
    let mut src_stalls = 0u32;
    let mut last_src_count = 0usize;
    let mut phase = if warm { 1u8 } else { 0u8 };
    let mut phase_deadline = if warm {
        started + Duration::from_secs(12)
    } else {
        started + Duration::from_secs(4)
    };

    loop {
        let now = Instant::now();
        if now >= total_deadline {
            break;
        }
        if now >= phase_deadline {
            match phase {
                0 => {
                    tracing::info!("[KAD] 引导完成: {} 个联系", session.contacts.len());
                    phase = 1;
                    let left = total_deadline.saturating_duration_since(Instant::now());
                    phase_deadline = Instant::now() + left / 2;
                    best_dist = None;
                }
                1 => {
                    tracing::info!(
                        "[KAD] FindNode 收敛: {} 联系 / {} 已查询,查源发往最近 12 节点",
                        session.contacts.len(),
                        session.queried.len()
                    );
                    phase = 2;
                    phase_deadline = total_deadline;
                    // 查源只发给 ver>=3 且已应答过的节点。
                    let targets = session.closest(12, |c| c.ver >= 3 && session.queried.contains(&c.id_wire));
                    if targets.is_empty() {
                        // 没有可用节点:退回引导重新来一轮。
                        phase = 0;
                        phase_deadline = Instant::now() + Duration::from_secs(8);
                        pending = bootstrap.clone();
                        sent_bootstrap.clear();
                        continue;
                    }
                    let mut p = Vec::new();
                    p.extend_from_slice(&raw_from_wire(&session.target_raw));
                    p.extend_from_slice(&0u16.to_le_bytes());
                    p.extend_from_slice(&size.to_le_bytes());
                    for t in &targets {
                        session.send(OP_SEARCH_SOURCE_REQ, &p, t);
                    }
                }
                _ => break,
            }
        }
        if phase == 0 {
            let mut sent = 0usize;
            for node in pending.iter() {
                if sent >= 12 {
                    break;
                }
                if sent_bootstrap.insert(node.id_wire) {
                    let mut bp = Vec::new();
                    bp.extend_from_slice(&raw_from_wire(&session.target_raw));
                    bp.extend_from_slice(&4662u16.to_le_bytes());
                    bp.push(KADEMLIA_BOOT_VER);
                    session.send(OP_BOOTSTRAP_REQ, &bp, node);
                    let mut hp = Vec::new();
                    hp.extend_from_slice(&raw_from_wire(&session.target_raw));
                    hp.extend_from_slice(&0u16.to_le_bytes());
                    hp.push(KADEMLIA_HELLO_VER);
                    hp.push(0);
                    session.send(OP_HELLO_REQ, &hp, node);
                    sent += 1;
                }
            }
            pending.retain(|n| !sent_bootstrap.contains(&n.id_wire));
            if pending.is_empty() && !session.contacts.is_empty() {
                phase_deadline = Instant::now();
            }
        } else if phase == 1 {
            let targets = session.closest(5, |c| !session.queried.contains(&c.id_wire));
            if targets.is_empty() {
                phase_deadline = Instant::now();
            }
            for t in &targets {
                session.queried.insert(t.id_wire);
                let mut p = Vec::new();
                p.push(3u8); // type: FindNode
                p.extend_from_slice(&raw_from_wire(&session.target_raw));
                p.extend_from_slice(&t.id_wire);
                session.send(OP_KAD_REQ, &p, t);
            }
        }
        // 收包窗口 2 秒。
        let mut got = false;
        let until = Instant::now() + Duration::from_millis(2000);
        loop {
            let now2 = Instant::now();
            if now2 >= until || now2 >= total_deadline {
                break;
            }
            match tokio::time::timeout(until - now2, session.socket.recv_from(&mut buf)).await {
                Ok(Ok((n, addr))) => {
                    session.on_packet(addr, &buf[..n]);
                    got = true;
                }
                _ => break,
            }
        }
        if !got {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if phase == 1 {
            // 距离不再收敛(3 轮无改进)就提前进入查源阶段。
            let cur = session
                .contacts
                .iter()
                .map(|c| xor_distance(&session.target_raw, &raw_from_wire(&c.id_wire)))
                .min();
            let improved = match (best_dist, cur) {
                (Some(prev), Some(now)) => now < prev,
                (_, Some(_)) => true,
                _ => false,
            };
            if improved {
                best_dist = cur;
                stalls = 0;
            } else {
                stalls += 1;
                if stalls >= 3 {
                    phase_deadline = Instant::now();
                }
            }
        }
        if phase == 2 {
            if session.sources.len() >= 8 {
                break;
            }
            if !session.sources.is_empty() {
                if session.sources.len() == last_src_count {
                    src_stalls += 1;
                    if src_stalls >= 2 {
                        break;
                    }
                } else {
                    src_stalls = 0;
                    last_src_count = session.sources.len();
                }
            }
        }
    }

    // 持久化联系人缓存(下次热启动)。
    let mut merged: Vec<KadNode> = Vec::new();
    let mut seen_ids: std::collections::HashSet<[u8; 16]> = Default::default();
    for n in session.contacts.into_iter().chain(cached_contacts) {
        if seen_ids.insert(n.id_wire) {
            merged.push(n);
        }
    }
    merged.truncate(600);
    if let Ok(b) = serde_json::to_vec(&merged) {
        cached::write_cache(&cache_file, &b);
    }
    if session.sources.is_empty() {
        return Err("KAD 网络未找到来源(节点不可达或文件无来源)".into());
    }
    Ok(session.sources)
}

pub async fn kad_enabled(app: &App) -> bool {
    app.db.setting(KAD_ENABLED_KEY).await.as_deref() != Some("0")
}

/// 管理后台展示字段。
pub async fn status_fields(app: &App) -> serde_json::Value {
    json!({
        "kad_enabled": kad_enabled(app).await,
        "kad_nodes_url": app
            .db
            .setting(KAD_NODES_URL_KEY)
            .await
            .unwrap_or_else(|| DEFAULT_NODES_URL.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nodes_dat_v1() {
        // 传统 0 头布局:num(4) + [id(16) + ip(4,反序) + udp(2) + udp(2) + ver(1)]
        let mut data: Vec<u8> = Vec::new();
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(&[1u8; 16]); // id
        data.extend_from_slice(&[1, 2, 3, 4]); // ip 反序存储
        data.extend_from_slice(&4661u16.to_le_bytes());
        data.extend_from_slice(&4661u16.to_le_bytes());
        data.push(8u8); // 版本 8
        let nodes = parse_nodes_dat(&data);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].ip, [4, 3, 2, 1]);
        assert_eq!(nodes[0].udp, 4661);
    }

    #[test]
    fn wire_reversal() {
        let wire = [1u8, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0, 4, 0, 0, 0];
        let raw = raw_from_wire(&wire);
        assert_eq!(raw, [0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0, 4]);
    }

    #[test]
    fn tag_parsing() {
        // 一个 UINT32 标签:count=1, ty=0x03, nlen=1, id=0xFE, value=0x7F000001
        let mut d = vec![1u8, 0x03, 0x01, 0x00, 0xFE];
        d.extend_from_slice(&0x7F000001u32.to_le_bytes());
        let mut off = 0usize;
        let tags = read_tag_list(&d, &mut off).unwrap();
        assert_eq!(tags[0].0, 0xFE);
        assert_eq!(tags[0].1.len(), 4);
    }
}
