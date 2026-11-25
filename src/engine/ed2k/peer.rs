//! eD2K 对等连接状态机:握手(HELLO)→ 文件协商(FileRequest / FileStatus)
//! → 申请上传槽(StartUploadReq / AcceptUploadReq)→ 请求块(RequestParts)
//! → 接收数据(SendingPart / CompressedPart)→ 校验块 MD4。
//!
//! 只发请求类帧,绝不上传数据(纯下载约束,由端到端测试断言)。

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::time::Duration;

use flate2::read::ZlibDecoder;
use std::io::Read;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufReader, SeekFrom};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;

use super::link::md4;

pub const PROTO_EDONKEY: u8 = 0xE3;
pub const PROTO_EMULE: u8 = 0xC5;
pub const PROTO_PACKED: u8 = 0xD4;

const OP_HELLO: u8 = 0x01;
const OP_SENDINGPART: u8 = 0x46;
const OP_REQUESTPARTS: u8 = 0x47;
const OP_COMPRESSEDPART: u8 = 0x40;
const OP_HELLOANSWER: u8 = 0x4C;
const OP_FILEREQANSNOFIL: u8 = 0x48;
const OP_FILESTATUS: u8 = 0x50;
const OP_HASHSETREQUEST: u8 = 0x51;
const OP_HASHSETANSWER: u8 = 0x52;
const OP_STARTUPLOADREQ: u8 = 0x54;
const OP_ACCEPTUPLOADREQ: u8 = 0x55;
const OP_OUTOFPARTREQS: u8 = 0x57;
const OP_QUEUERANKING: u8 = 0x60;
const OP_SETREQFILEID: u8 = 0x4F;
const OP_QUEUERANK: u8 = 0x5C;
const OP_COMPRESSEDPART_I64: u8 = 0xA1;
const OP_SENDINGPART_I64: u8 = 0xA2;
const OP_REQUESTPARTS_I64: u8 = 0xA3;

/// eD2K 协议的 9.28MB 分块大小(哈希与请求块都以它为界)。
pub const PART_SIZE: u64 = 9_728_000;
/// 单个块内请求子区间的常用大小(180KB,对齐主流客户端)。
const REQ_CHUNK: u64 = 184_320;
/// 同一块内同时在途的子请求个数(协议上限 3)。
const MAX_INFLIGHT: usize = 3;
const MAX_PEER_FRAME: u32 = REQ_CHUNK as u32 * 4 + 4096;
const PEER_STALL_TIMEOUT: Duration = Duration::from_secs(60);
const QUEUE_TIMEOUT: Duration = Duration::from_secs(120);
const MD4_EMPTY: [u8; 16] = [
    0x31, 0xd6, 0xcf, 0xe0, 0xd1, 0x6a, 0xe9, 0x31, 0xb7, 0x3c, 0x59, 0xd7, 0xe0, 0xc0, 0x89, 0xc0,
];

/// u32 偏移能表达的文件上限(PART_SIZE 的整数倍)。
fn max_u32_file() -> u64 {
    (u32::MAX as u64 / PART_SIZE) * PART_SIZE
}

pub fn part_count(size: i64) -> usize {
    if size <= 0 {
        return 1;
    }
    ((size.max(0) as u64).div_ceil(PART_SIZE)) as usize
}

/// 第 `part` 块的 `(start, end)` 绝对偏移区间。
pub fn part_span(part: usize, size: i64) -> (u64, u64) {
    let start = part as u64 * PART_SIZE;
    let end = (size as u64).min(start + PART_SIZE).max(start);
    (start, end)
}

/// 校验并规范化对端发来的 hashset(含 root 校验,防投毒)。
pub fn normalize_hashset(net: &[[u8; 16]], root: &[u8; 16], size: i64) -> Option<Vec<[u8; 16]>> {
    let size = size.max(0) as u64;
    let pc = part_count(size as i64);
    let mut list = net.to_vec();
    if size < PART_SIZE {
        return if list.len() == 1 && list[0] == *root { Some(list) } else { None };
    }
    // 部分客户端会在末尾附加空文件的 MD4。
    if list.len() == pc + 1 && size.is_multiple_of(PART_SIZE) && list[pc] == MD4_EMPTY {
        list.truncate(pc);
    }
    if list.len() != pc {
        return None;
    }
    let mut concat = Vec::with_capacity(list.len() * 16);
    for h in &list {
        concat.extend_from_slice(h);
    }
    let ok = if size.is_multiple_of(PART_SIZE) {
        concat.extend_from_slice(&MD4_EMPTY);
        md4(&concat) == *root
    } else {
        md4(&concat) == *root
    };
    ok.then_some(list)
}

async fn read_frame(rd: &mut BufReader<OwnedReadHalf>) -> Result<(u8, u8, Vec<u8>), String> {
    let mut head = [0u8; 6];
    rd.read_exact(&mut head).await.map_err(|e| format!("连接断开: {e}"))?;
    let proto = head[0];
    if proto != PROTO_EDONKEY && proto != PROTO_EMULE && proto != PROTO_PACKED {
        return Err(format!("非法协议字节 0x{proto:02X}"));
    }
    let size = u32::from_le_bytes([head[1], head[2], head[3], head[4]]);
    if size == 0 {
        return Err("空帧".into());
    }
    let payload_len = size as usize - 1;
    if payload_len > MAX_PEER_FRAME as usize {
        return Err(format!("帧过大 {payload_len}"));
    }
    let opcode = head[5];
    let mut payload = vec![0u8; payload_len];
    rd.read_exact(&mut payload).await.map_err(|e| format!("读帧失败: {e}"))?;
    let payload = if proto == PROTO_PACKED {
        let mut dec = ZlibDecoder::new(&payload[..]);
        let mut out = Vec::new();
        dec.read_to_end(&mut out).map_err(|e| format!("解压失败: {e}"))?;
        if out.len() > MAX_PEER_FRAME as usize {
            return Err("解压超限".into());
        }
        out
    } else {
        payload
    };
    Ok((proto, opcode, payload))
}

fn frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 6);
    out.push(PROTO_EDONKEY);
    out.extend_from_slice(&((payload.len() + 1) as u32).to_le_bytes());
    out.push(opcode);
    out.extend_from_slice(payload);
    out
}

fn frame_emule(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 6);
    out.push(PROTO_EMULE);
    out.extend_from_slice(&((payload.len() + 1) as u32).to_le_bytes());
    out.push(opcode);
    out.extend_from_slice(payload);
    out
}

fn peer_tag_str(id: u8, value: &str) -> Vec<u8> {
    let mut v = vec![0x02u8];
    v.extend_from_slice(&1u16.to_le_bytes());
    v.push(id);
    v.extend_from_slice(&(value.len() as u16).to_le_bytes());
    v.extend_from_slice(value.as_bytes());
    v
}

fn peer_tag_u32(id: u8, value: u32) -> Vec<u8> {
    let mut v = vec![0x03u8];
    v.extend_from_slice(&1u16.to_le_bytes());
    v.push(id);
    v.extend_from_slice(&value.to_le_bytes());
    v
}

fn hello_payload() -> Vec<u8> {
    let user_hash = {
        use std::sync::OnceLock;
        static H: OnceLock<[u8; 16]> = OnceLock::new();
        *H.get_or_init(|| md4(format!("ndisk-peer-{}", std::process::id()).as_bytes()))
    };
    let mut p = Vec::new();
    p.push(0x10u8); // hash 长度 16
    p.extend_from_slice(&user_hash);
    p.extend_from_slice(&0u32.to_le_bytes()); // id(占位)
    p.extend_from_slice(&0u16.to_le_bytes()); // 端口(占位)
    p.extend_from_slice(&4u32.to_le_bytes()); // 标签数
    p.extend_from_slice(&peer_tag_str(0x01, "NDisk"));
    p.extend_from_slice(&peer_tag_u32(0x11, 0x3C)); // 版本 60
    p.extend_from_slice(&peer_tag_u32(0xFB, (3 << 24) | (1 << 7))); // 兼容标志
    p.extend_from_slice(&peer_tag_u32(0xFE, 1 << 4)); // 扩展标志
    p.extend_from_slice(&0u32.to_le_bytes()); // 服务器 ip(占位)
    p.extend_from_slice(&0u16.to_le_bytes()); // 服务器端口(占位)
    p
}

pub enum PeerErr {
    /// 对端声明没有这个文件。
    NoFile,
    /// 协议违规 / 数据与声明不符(可信度问题)。
    Integrity(String),
    /// 网络与超时类错误。
    Net(String),
}

impl std::fmt::Display for PeerErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PeerErr::NoFile => write!(f, "对端没有此文件"),
            PeerErr::Integrity(m) => write!(f, "完整性违规: {m}"),
            PeerErr::Net(m) => write!(f, "{m}"),
        }
    }
}

impl From<String> for PeerErr {
    fn from(e: String) -> Self {
        PeerErr::Net(e)
    }
}

/// 一条已经"拿到上传槽"的对端连接,可以请求 hashset 与分片数据。
pub struct PeerSession {
    pub addr: SocketAddr,
    rd: BufReader<OwnedReadHalf>,
    wr: OwnedWriteHalf,
    /// 对端拥有的块位图(空 = 假定全有)。
    bitfield: Vec<u8>,
    hash: [u8; 16],
    size: i64,
    /// 文件超过 u32 偏移上限时使用 64 位协议变体。
    large: bool,
}

impl PeerSession {
    /// 建立连接并完成 握手 → 文件协商 → 申请上传槽 三步。
    pub async fn open(addr: SocketAddr, hash: &[u8; 16], size: i64) -> Result<PeerSession, PeerErr> {
        let stream = tokio::time::timeout(Duration::from_secs(10), TcpStream::connect(addr))
            .await
            .map_err(|_| PeerErr::Net("连接超时".into()))?
            .map_err(|e| PeerErr::Net(format!("连接失败: {e}")))?;
        let (rd, mut wr) = stream.into_split();
        let mut rd = BufReader::new(rd);
        let large = (size.max(0) as u64) > max_u32_file();

        tracing::info!("[eD2K peer] {} 连接,发 HELLO", addr);
        wr.write_all(&frame(OP_HELLO, &hello_payload()))
            .await
            .map_err(|e| PeerErr::Net(format!("发送失败: {e}")))?;
        let t0 = tokio::time::Instant::now() + PEER_STALL_TIMEOUT;
        loop {
            let left = t0.saturating_duration_since(tokio::time::Instant::now());
            if left.is_zero() {
                return Err(PeerErr::Net("握手超时".into()));
            }
            match tokio::time::timeout(left, read_frame(&mut rd)).await {
                Ok(Ok((_, op, _))) => {
                    if op == OP_HELLOANSWER {
                        break;
                    }
                }
                Ok(Err(e)) => return Err(PeerErr::Net(e)),
                Err(_) => return Err(PeerErr::Net("握手超时".into())),
            }
        }

        tracing::info!("[eD2K peer] {} HELLO 应答已收,发 FileRequest", addr);
        wr.write_all(&frame(0x58, hash)).await.map_err(|e| PeerErr::Net(format!("发送失败: {e}")))?;
        let mut bitfield = Vec::new();
        let mut got_status = false;
        let mut sent_setreq = false;
        for _ in 0..16 {
            let (_, op, payload) = tokio::time::timeout(PEER_STALL_TIMEOUT, read_frame(&mut rd))
                .await
                .map_err(|_| PeerErr::Net("文件协商超时".into()))?
                .map_err(PeerErr::Net)?;
            match op {
                // 部分客户端要求先 SetReqFileId 才回 FileStatus。
                0x59 => {
                    if !sent_setreq {
                        wr.write_all(&frame(OP_SETREQFILEID, hash))
                            .await
                            .map_err(|e| PeerErr::Net(format!("发送失败: {e}")))?;
                        sent_setreq = true;
                    }
                }
                OP_FILEREQANSNOFIL => return Err(PeerErr::NoFile),
                OP_FILESTATUS => {
                    if payload.len() >= 18 {
                        let pc = u16::from_le_bytes([payload[16], payload[17]]) as usize;
                        let nbytes = pc.div_ceil(8);
                        if let Some(bf) = payload.get(18..18 + nbytes) {
                            bitfield = bf.to_vec();
                        }
                    }
                    got_status = true;
                    break;
                }
                _ => {}
            }
        }
        if !got_status {
            return Err(PeerErr::Net("未收到文件状态".into()));
        }

        tracing::info!("[eD2K peer] {} 文件状态已收(位图 {} 字节),申请上传槽", addr, bitfield.len());
        wr.write_all(&frame(OP_STARTUPLOADREQ, hash))
            .await
            .map_err(|e| PeerErr::Net(format!("发送失败: {e}")))?;
        let t1 = tokio::time::Instant::now() + QUEUE_TIMEOUT;
        loop {
            let left = t1.saturating_duration_since(tokio::time::Instant::now());
            if left.is_zero() {
                return Err(PeerErr::Net("上传排队超时".into()));
            }
            match tokio::time::timeout(left, read_frame(&mut rd)).await {
                Ok(Ok((_, op, payload))) => match op {
                    OP_ACCEPTUPLOADREQ => break,
                    OP_QUEUERANKING | OP_QUEUERANK => {}
                    OP_OUTOFPARTREQS => return Err(PeerErr::Net("对端上传队列已满".into())),
                    OP_FILESTATUS if payload.len() >= 18 => {
                        let pc = u16::from_le_bytes([payload[16], payload[17]]) as usize;
                        let nbytes = pc.div_ceil(8);
                        if let Some(bf) = payload.get(18..18 + nbytes) {
                            bitfield = bf.to_vec();
                        }
                    }
                    _ => {}
                },
                Ok(Err(e)) => return Err(PeerErr::Net(e)),
                Err(_) => return Err(PeerErr::Net("上传排队超时".into())),
            }
        }

        tracing::info!("[eD2K peer] {} 上传槽已获取", addr);
        Ok(Self { addr, rd, wr, bitfield, hash: *hash, size, large })
    }

    /// 对端是否拥有第 `part` 块(位图为空时假定全有)。
    pub fn has_part(&self, part: usize, parts_total: usize) -> bool {
        if part >= parts_total {
            return false;
        }
        if self.bitfield.is_empty() {
            return true;
        }
        self.bitfield.get(part / 8).is_some_and(|byte| byte & (1 << (part % 8)) != 0)
    }

    /// 请求并接收块哈希表(root 校验防投毒)。
    pub async fn fetch_hashset(&mut self) -> Result<Vec<[u8; 16]>, PeerErr> {
        tracing::info!("[eD2K peer] {} 请求 hashset", self.addr);
        self.wr
            .write_all(&frame(OP_HASHSETREQUEST, &self.hash))
            .await
            .map_err(|e| PeerErr::Net(format!("发送失败: {e}")))?;
        for _ in 0..16 {
            let (_, op, payload) = tokio::time::timeout(PEER_STALL_TIMEOUT, read_frame(&mut self.rd))
                .await
                .map_err(|_| PeerErr::Net("hashset 超时".into()))?
                .map_err(PeerErr::Net)?;
            if op != OP_HASHSETANSWER {
                continue;
            }
            // 有的实现带根哈希前缀,有的不带。
            let off = if payload.len() >= 18 && payload[0..16] == self.hash { 16 } else { 0 };
            if payload.len() < off + 2 {
                return Err(PeerErr::Integrity("hashset 过短".into()));
            }
            let count = u16::from_le_bytes([payload[off], payload[off + 1]]) as usize;
            if count == 0 || count > 40000 || payload.len() < off + 2 + count * 16 {
                return Err(PeerErr::Integrity("hashset 计数异常".into()));
            }
            let mut hashes = Vec::with_capacity(count);
            for i in 0..count {
                let base = off + 2 + i * 16;
                if payload.len() < base + 16 {
                    return Err(PeerErr::Integrity("hashset 数据不完整".into()));
                }
                let mut h = [0u8; 16];
                h.copy_from_slice(&payload[base..base + 16]);
                hashes.push(h);
            }
            return normalize_hashset(&hashes, &self.hash, self.size)
                .ok_or_else(|| PeerErr::Integrity("hashset root 校验失败(投毒?)".into()));
        }
        Err(PeerErr::Net("未收到 hashset".into()))
    }

    /// 下载一整块:内部以 ≤3 个子请求在途、乱序缓冲、前缀推进的方式收满
    /// `[part_start, part_end)`,返回整块 MD4(调用方与 hashset 比对)。
    pub async fn download_part(
        &mut self,
        part: usize,
        file: &tokio::sync::Mutex<tokio::fs::File>,
        received: &std::sync::atomic::AtomicU64,
    ) -> Result<[u8; 16], PeerErr> {
        let (part_start, part_end) = part_span(part, self.size);
        let part_len = (part_end - part_start) as usize;
        let mut hashbuf = vec![0u8; part_len];

        let mut inflight: VecDeque<(u64, u64)> = VecDeque::new();
        // prefix 之前的数据已连续收满;乱序到达的区间先暂存 lagged。
        let mut prefix: u64 = part_start;
        let mut lagged: Vec<(u64, u64)> = Vec::new();
        let mut resend = false;
        let mut stall = tokio::time::Instant::now() + PEER_STALL_TIMEOUT;

        loop {
            // 用乱序缓冲推进 prefix(可多轮)。
            loop {
                let before = prefix;
                lagged.sort();
                let mut merged: Vec<(u64, u64)> = Vec::new();
                for (s, e) in &lagged {
                    if *s <= prefix {
                        prefix = prefix.max(*e);
                    } else if let Some(last) = merged.last_mut() {
                        if *s <= last.1 {
                            last.1 = last.1.max(*e);
                        } else {
                            merged.push((*s, *e));
                        }
                    } else {
                        merged.push((*s, *e));
                    }
                }
                lagged = merged;
                inflight.retain(|(_s, e)| *e > prefix);
                if prefix == before {
                    break;
                }
            }
            if prefix >= part_end {
                break;
            }

            // 补充在途子请求,直到 3 个或到块尾;跳过已被覆盖的区间。
            let before = inflight.len();
            let mut covered: Vec<(u64, u64)> = inflight.iter().copied().collect();
            covered.extend_from_slice(&lagged);
            covered.sort();
            let mut cursor = prefix;
            let mut added: Vec<(u64, u64)> = Vec::new();
            while inflight.len() + added.len() < MAX_INFLIGHT && cursor < part_end {
                let overlaps = covered.iter().any(|(s, e)| cursor < *e && cend_of(cursor, part_end) > *s);
                let cend = (cursor + REQ_CHUNK).min(part_end);
                if !overlaps {
                    inflight.push_back((cursor, cend));
                    added.push((cursor, cend));
                    covered.push((cursor, cend));
                    covered.sort();
                }
                cursor = cend;
            }
            if (inflight.len() != before && !added.is_empty()) || resend {
                // 协议固定 3 组 (start,end);不足的组填 0。
                let mut starts = [0u64; 3];
                let mut ends = [0u64; 3];
                for (i, (s, e)) in inflight.iter().take(3).enumerate() {
                    starts[i] = *s;
                    ends[i] = *e;
                }
                let mut built = Vec::with_capacity(64);
                built.extend_from_slice(&self.hash);
                if self.large {
                    for s in starts {
                        built.extend_from_slice(&s.to_le_bytes());
                    }
                    for e in ends {
                        built.extend_from_slice(&e.to_le_bytes());
                    }
                    self.wr
                        .write_all(&frame_emule(OP_REQUESTPARTS_I64, &built))
                        .await
                        .map_err(|e| PeerErr::Net(format!("发送失败: {e}")))?;
                } else {
                    for s in starts {
                        built.extend_from_slice(&(s as u32).to_le_bytes());
                    }
                    for e in ends {
                        built.extend_from_slice(&(e as u32).to_le_bytes());
                    }
                    self.wr
                        .write_all(&frame(OP_REQUESTPARTS, &built))
                        .await
                        .map_err(|e| PeerErr::Net(format!("发送失败: {e}")))?;
                }
                resend = false;
            }
            if inflight.is_empty() {
                return Err(PeerErr::Net("分片请求无法推进".into()));
            }

            let (_, op, payload) = match tokio::time::timeout(
                stall.saturating_duration_since(tokio::time::Instant::now()),
                read_frame(&mut self.rd),
            )
            .await
            {
                Ok(Ok(v)) => v,
                Ok(Err(e)) => return Err(PeerErr::Net(e)),
                Err(_) => return Err(PeerErr::Net("传输停滞".into())),
            };

            let mut got_data = false;
            match op {
                OP_SENDINGPART if payload.len() >= 24 => {
                    let start = u32::from_le_bytes([payload[16], payload[17], payload[18], payload[19]]) as u64;
                    let end = u32::from_le_bytes([payload[20], payload[21], payload[22], payload[23]]) as u64;
                    self.absorb(start, end, &payload[24..], &mut prefix, &mut lagged, &mut inflight, &mut hashbuf, part_start, file, received)
                        .await?;
                    got_data = true;
                }
                OP_SENDINGPART_I64 if payload.len() >= 32 => {
                    let start = u64::from_le_bytes(payload[16..24].try_into().unwrap());
                    let end = u64::from_le_bytes(payload[24..32].try_into().unwrap());
                    self.absorb(start, end, &payload[32..], &mut prefix, &mut lagged, &mut inflight, &mut hashbuf, part_start, file, received)
                        .await?;
                    got_data = true;
                }
                OP_COMPRESSEDPART if payload.len() >= 24 => {
                    let start = u32::from_le_bytes([payload[16], payload[17], payload[18], payload[19]]) as u64;
                    let data = decompress_bounded(&payload[24..], (REQ_CHUNK * 2) as usize)?;
                    let end = start + data.len() as u64;
                    self.absorb(start, end, &data, &mut prefix, &mut lagged, &mut inflight, &mut hashbuf, part_start, file, received)
                        .await?;
                    got_data = true;
                }
                OP_COMPRESSEDPART_I64 if payload.len() >= 28 => {
                    let start = u64::from_le_bytes(payload[16..24].try_into().unwrap());
                    let data = decompress_bounded(&payload[28..], (REQ_CHUNK * 2) as usize)?;
                    let end = start + data.len() as u64;
                    self.absorb(start, end, &data, &mut prefix, &mut lagged, &mut inflight, &mut hashbuf, part_start, file, received)
                        .await?;
                    got_data = true;
                }
                OP_FILESTATUS if payload.len() >= 18 => {
                    let pc = u16::from_le_bytes([payload[16], payload[17]]) as usize;
                    let nbytes = pc.div_ceil(8);
                    if let Some(bf) = payload.get(18..18 + nbytes) {
                        self.bitfield = bf.to_vec();
                    }
                }
                OP_FILEREQANSNOFIL => return Err(PeerErr::NoFile),
                // 上传槽被回收:重新排队申请。
                OP_OUTOFPARTREQS => {
                    self.wr
                        .write_all(&frame(OP_STARTUPLOADREQ, &self.hash))
                        .await
                        .map_err(|e| PeerErr::Net(format!("发送失败: {e}")))?;
                    let t = tokio::time::Instant::now() + QUEUE_TIMEOUT;
                    loop {
                        let left = t.saturating_duration_since(tokio::time::Instant::now());
                        if left.is_zero() {
                            return Err(PeerErr::Net("上传排队超时".into()));
                        }
                        match tokio::time::timeout(left, read_frame(&mut self.rd)).await {
                            Ok(Ok((_, op2, _))) => {
                                if op2 == OP_ACCEPTUPLOADREQ {
                                    break;
                                }
                            }
                            Ok(Err(e)) => return Err(PeerErr::Net(e)),
                            Err(_) => return Err(PeerErr::Net("上传排队超时".into())),
                        }
                    }
                    resend = true;
                }
                OP_QUEUERANKING => {}
                _ => {}
            }
            if got_data {
                stall = tokio::time::Instant::now() + PEER_STALL_TIMEOUT;
            }
            if prefix >= part_end {
                break;
            }
        }

        {
            let mut f = file.lock().await;
            f.flush().await.map_err(|e| PeerErr::Net(format!("写盘失败: {e}")))?;
        }
        let md4 = tokio::task::spawn_blocking(move || md4(&hashbuf))
            .await
            .map_err(|e| PeerErr::Net(format!("md4 join: {e}")))?;
        Ok(md4)
    }

    #[allow(clippy::too_many_arguments)]
    async fn absorb(
        &mut self,
        start: u64,
        end: u64,
        data: &[u8],
        prefix: &mut u64,
        lagged: &mut Vec<(u64, u64)>,
        inflight: &mut VecDeque<(u64, u64)>,
        hashbuf: &mut [u8],
        part_start: u64,
        file: &tokio::sync::Mutex<tokio::fs::File>,
        received: &std::sync::atomic::AtomicU64,
    ) -> Result<(), PeerErr> {
        // 一切越界 / 长度不符 / 与前缀重叠的数据都按完整性违规处理。
        let pe = (self.size.max(0) as u64).min(start + PART_SIZE).max(start);
        if end <= start || end > pe {
            return Err(PeerErr::Integrity(format!("分片越界 [{start},{end})")));
        }
        if data.len() as u64 != end - start {
            return Err(PeerErr::Integrity("分片长度与声明不符".into()));
        }
        if start < *prefix {
            if end <= *prefix {
                return Ok(());
            }
            return Err(PeerErr::Integrity("分片与前缀重叠".into()));
        }
        let rel = (start - part_start) as usize;
        hashbuf[rel..rel + data.len()].copy_from_slice(data);
        {
            let mut f = file.lock().await;
            f.seek(SeekFrom::Start(start)).await.map_err(|e| PeerErr::Net(format!("seek 失败: {e}")))?;
            f.write_all(data).await.map_err(|e| PeerErr::Net(format!("写盘失败: {e}")))?;
        }
        received.fetch_add(data.len() as u64, std::sync::atomic::Ordering::Relaxed);
        if start == *prefix {
            *prefix = end;
            inflight.retain(|(s, _)| *s != start);
        } else {
            lagged.push((start, end));
        }
        Ok(())
    }
}

fn cend_of(cursor: u64, part_end: u64) -> u64 {
    (cursor + REQ_CHUNK).min(part_end)
}

fn decompress_bounded(data: &[u8], max: usize) -> Result<Vec<u8>, PeerErr> {
    let dec = ZlibDecoder::new(data);
    let mut out = Vec::new();
    dec.take(max as u64).read_to_end(&mut out).map_err(|e| PeerErr::Net(format!("zlib 解压失败: {e}")))?;
    if out.len() > max {
        return Err(PeerErr::Integrity("zlib 解压超限(炸弹?)".into()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn part_math() {
        assert_eq!(part_count(0), 1);
        assert_eq!(part_count(PART_SIZE as i64), 1);
        assert_eq!(part_count(PART_SIZE as i64 + 1), 2);
        assert_eq!(part_span(1, PART_SIZE as i64 + 5), (PART_SIZE, PART_SIZE + 5));
        assert_eq!(part_span(0, 100), (0, 100));
    }

    #[test]
    fn hashset_root_for_small_file() {
        let root = md4(b"ab");
        let ok = normalize_hashset(&[root], &root, 2);
        assert!(ok.is_some());
        let bad = normalize_hashset(&[[0u8; 16]], &root, 2);
        assert!(bad.is_none());
    }

    #[test]
    fn hashset_with_trailing_empty() {
        let parts = [md4(b"11111111"), md4(b"22222222")]; // 2 块
        let mut concat = Vec::new();
        concat.extend_from_slice(&parts[0]);
        concat.extend_from_slice(&parts[1]);
        concat.extend_from_slice(&MD4_EMPTY);
        let root = md4(&concat);
        // len == pc+1 且带空块结尾 → 截断后应通过。
        let ok = normalize_hashset(&[parts[0], parts[1], MD4_EMPTY], &root, (PART_SIZE * 2) as i64);
        assert!(ok.is_some());
    }
}
