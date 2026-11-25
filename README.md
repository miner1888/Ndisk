# NDisk — 多用户网盘

单文件部署的多用户网盘服务(Rust + axum + SQLite,前端内嵌,无外部依赖)。

## 功能

**文件与空间**
- 每个账号独立的个人空间目录;管理员可发布共享目录(全服可见)
- 文件 / 文件夹浏览、新建、重命名、移动、复制、删除、搜索
- 大文件分片上传:断点续传、秒传校验、磁盘空间预检
- 下载支持断点续传(Range / 206),多选打包下载(流式 ZIP,恒定内存)
- 文本文件在线查看与编辑;图片 / 音视频 / PDF / Office 文档在线预览

**分享与访问**
- 分享链接:有效期、提取码、权限(只读 / 可上传)、限速、匿名可访问
- 直链下载:HMAC 签名票据,免登录、可设过期
- WebDAV:`/dav/`,全动词实现,可映射为 Windows 网络驱动器;单次 PUT 上限 256MB

**权限与账号**
- 三级权限模型:游客 / 登录用户 / 管理员;目录级规则(私有 / 只读 / 读写),深路径生效
- 账号能力开关:WebDAV、分享、离线下载可按账号禁用
- 登录验证码、argon2 口令哈希、会话 48 小时过期

**媒体**
- 视频在线播放:非 MP4 容器自动转码(H.264/AAC MP4);原生 MP4 直接播放
- 上传后自动转码替换:AVI/MKV 等上传完成即转 MP4 并**替换原文件**(默认**关闭**,
  需在后台「系统维护」开启;开启后转码失败仍保留原件,不影响其它文件)
- 视频封面图、图片缩略图自动生成(需 ffmpeg;缺失时仅影响媒体功能)
- 派生缓存(f.mp4 / 封面 / 缩略图)30 天未被访问自动回收,源文件变动立即回收;
  **用户上传的文件本身永不自动删除**,缓存整目录删除也会按需重建

**离线下载**(下载到 `个人空间/下载`)
- HTTP(S) 直链:多段并行(线程数后台可调 1-32,默认 8)、断点续传、进度 SSE 推送
- 磁力 / BT:librqbit 引擎,.torrent 文件可直接上传,.torrent 直链自动识别;
  默认纯下载不上传(后台可开做种)
- 迅雷(thunder)链接自动解码;eD2K:服务器查源 + KAD 引导,纯下载不共享
- 单任务大小上限、内网地址防护,均可在后台调整
- 服务重启会中断进行中的任务并统一置为失败(不自动续传),临时文件 24h 回收

**系统维护(管理后台)**
- 用户管理 / 共享目录 / 目录权限 / 后台设置
- HTTPS 证书上传与热启停(不停服务切换 HTTP ↔ HTTPS)
- KAD 引导节点、BT tracker 订阅(均可填多个地址并行拉取)、eD2K 服务器(订阅缓存 24h 刷新)
- 缓存清理、数据库维护

## 快速开始

```bash
cargo build --release
./target/release/ndisk          # 默认 0.0.0.0:9000
```

初始管理员 `admin / admin123`(登录后请修改)。需要媒体功能时安装 ffmpeg,
或用环境变量 `NDISK_FFMPEG` 指定路径;日志级别用 `RUST_LOG` 调整。

| 参数 | 含义 | 默认值 |
| --- | --- | --- |
| `-l, --listen <addr>` | 监听地址 | `0.0.0.0:9000` |
| `-p, --port <port>` | 监听端口 | `9000` |
| `-H, --host <addr>` | 监听网卡 | `0.0.0.0` |
| `-t, --tls-listen <addr>` | HTTPS 监听 | `0.0.0.0:443` |
| `--tls-port <port>` | HTTPS 端口 | `443` |
| `-d, --data <dir>` | 数据目录 | `./data` |
| `-u, --admin-user <name>` | 初始管理员 | `admin` |
| `-w, --admin-pass <pw>` | 初始管理员密码 | `admin123` |
| `-c, --chunk-size <size>` | 上传分片大小 | `8M` |
| `-h / -V` | 帮助 / 版本 | |

短参数紧贴取值(`-p8080`),长参数用 `=`(`--port=8080`)。数据全部落在一个
`data/` 目录(`app.sqlite3` 主库 + 用户空间 + 分片临时 + 媒体缓存),迁移即拷贝。
systemd 部署示例见 [examples/](examples/),环境变量样例见 `examples/env.example`。

## 测试

```bash
cargo test                                # 单元测试 51 项
python tests/smoke.py                     # 端到端:API / 权限 / 上传下载 / ZIP / 分享 / WebDAV(42 项)
python tests/permissions_smoke.py         # 权限模型(19 项)
python tests/tls_smoke.py                 # HTTPS 热启停(11 项,需 python cryptography)
python tests/media_smoke.py               # 媒体(17 项,需 ffmpeg)
python tests/offline_http_smoke.py        # HTTP 直链离线 + 磁力/种子(22 项)
python tests/ed2k_local_smoke.py          # eD2K(9 项,本地 mock)
python tests/bt_local_smoke.py            # BT(需 mktorrent / transmission-cli,缺失跳过)
```

全部走回环或本地 mock,不依赖公网;默认测 debug 二进制(自动识别 `.exe`)。

## 依赖说明

**本版本不带 `vendor/`**:除 BT 引擎的 rqbit fork(`[patch.crates-io]` 指向固定 git rev)
外,全部依赖直接取自 crates.io,源码包约 0.7 MB,拷走即可构建(需联网拉依赖)。

已做的版本收敛(不需要 fork,只改本侧声明):

- argon2 0.6 / rand 0.10 / base64 0.23 / sha2 0.11 / reqwest 0.13 全部取最新,
  与依赖树其余部分对齐;argon2 0.6 换掉旧 blake2/digest 0.10 链。
- librqbit 以 `default-features = false` 重开 `disable-upload + rust-tls + http-api-client`,
  剪掉 `default-tls`(会拉 crypto-hash → hex 0.3 / openssl / winapi)。

剩余同名多版本是各 crate 自己声明的硬约束(向新版迁移需要改上游 manifest),
在不 fork 的前提下无法消除;清单与来源用 `python tools/dup_report.py` 随时可查。

分配器:全局 mimalloc,显式启用 `no_thp` 特性(仅 Linux/Android 生效)。
mimalloc v3 默认对堆内存 `madvise(MADV_HUGEPAGE)`,小对象页落入 2MB 大页后
释放不归还 OS,大文件下载/多线程分配会把常驻内存抬到数百 MB 且不回落;
关闭 THP 后 Linux 常驻内存与 Windows 一致(下载期间与完成后均在几十 MB 内)。

打包干净源码:`python tools/pack_src.py`(排除 target / data / 日志等,并自检
`include_bytes!`/`include_str!` 依赖是否完整)。

release 产物策略:`strip + LTO + codegen-units=1 + opt-level="z"`(二进制约 15 MB),
**panic 保持 unwind**——单个请求/任务的 panic 只断当前连接,进程继续服务;
后台 worker(离线下载/转码/维护循环)另有 panic 兜底,会把任务置为 failed 而不是
永远停在 running。

